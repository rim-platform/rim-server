use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;
use sea_orm::DatabaseConnection;

use rim_core::conversation::model::Conversation;
use rim_core::message::model::{Message, MessageId};
use rim_core::message_reaction::model::{MessageReaction, NewMessageReaction};
use rim_core::relay::repository::RelayRepository;
use rim_core::server::model::ServerId;
use rim_core::server_feature::model::ServerFeature;
use rim_core::server_member::model::MemberStanding;
use rim_infra_db::TransactionScope;
use rim_infra_db::begin_transaction;
use rim_services::events::{EventObserver, Observers, ReactionGroupData, WorkspaceEvent};
use rim_services::feature::service::FeatureGate;
use rim_services::message_reaction::command::{
    GetMessageReactionsQuery, GetReactionsForMessagesQuery, ToggleReactionCmd,
};
use rim_services::message_reaction::dto::ReactionGroupDto;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::notification::command::NotifyForReactionCmd;
use rim_services::notification::service::NotificationService;
use rim_services::{ServiceError, ServiceResult};

use crate::ReactionRepos;
use crate::repos::AppRepos;
use crate::request_gate::ensure_request_decided;
use crate::util::{
    ConversationAccessRepos, PostKind, ensure_can_post, ensure_conversation_in_server,
    ensure_conversation_member_access, ensure_conversation_writable_with_standing,
};

/// Most distinct emoji one message carries.
pub(crate) const MAX_REACTION_GROUPS: usize = 20;
/// Most reactions one user puts on one message.
pub(crate) const MAX_REACTIONS_PER_USER: usize = 10;
/// Longest reaction, in characters.
const MAX_EMOJI_CHARS: usize = 32;

/// A Unicode emoji: pictographs and the components that join, modify or tag them (zero-width
/// joiner, variation selectors, skin tones, hair, keycaps, regional indicators, tags), with at
/// least one pictograph, regional indicator or keycap, so digits, `#` and `*` alone do not pass.
static UNICODE_EMOJI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\p{Extended_Pictographic}\p{Emoji_Component}\x{FE0E}]*[\p{Extended_Pictographic}\p{Regional_Indicator}\x{20E3}][\p{Extended_Pictographic}\p{Emoji_Component}\x{FE0E}]*$",
    )
    .expect("valid regex")
});

/// A custom emoji by name, `:name:`, the name made of letters, digits and underscores as custom
/// emoji names are.
static EMOJI_SHORTCODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^:[\p{Alphabetic}\p{N}_]+:$").expect("valid regex"));

/// The trimmed reaction when it is 1 to 32 characters long, whatever its content.
///
/// # Errors
/// `Validation` when it is empty or longer.
fn trim_reaction(emoji: &str) -> ServiceResult<&str> {
    let emoji = emoji.trim();
    if emoji.is_empty() {
        return Err(ServiceError::Validation(
            "emoji must be non-empty".to_string(),
        ));
    }
    if emoji.chars().count() > MAX_EMOJI_CHARS {
        return Err(ServiceError::Validation("emoji too long".to_string()));
    }
    Ok(emoji)
}

/// The trimmed reaction when it is a Unicode emoji or a `:name:` shortcode of at most 32
/// characters.
///
/// # Errors
/// `Validation` for anything else.
pub(crate) fn validate_reaction_emoji(emoji: &str) -> ServiceResult<String> {
    let emoji = trim_reaction(emoji)?;
    if !UNICODE_EMOJI.is_match(emoji) && !EMOJI_SHORTCODE.is_match(emoji) {
        return Err(ServiceError::Validation(
            "a reaction must be an emoji or a :name: shortcode".to_string(),
        ));
    }
    Ok(emoji.to_string())
}

/// `Validation` when adding `emoji` by `user_id` to a message holding `reactions` would pass
/// [`MAX_REACTION_GROUPS`] distinct emoji or [`MAX_REACTIONS_PER_USER`] reactions by the user.
fn ensure_room_for_reaction(
    reactions: &[MessageReaction],
    user_id: rim_core::user::model::UserId,
    emoji: &str,
) -> ServiceResult<()> {
    let groups: HashSet<&str> = reactions.iter().map(|r| r.emoji.as_str()).collect();
    if !groups.contains(emoji) && groups.len() >= MAX_REACTION_GROUPS {
        return Err(ServiceError::Validation(format!(
            "a message carries at most {MAX_REACTION_GROUPS} different reactions"
        )));
    }
    if reactions.iter().filter(|r| r.user_id == user_id).count() >= MAX_REACTIONS_PER_USER {
        return Err(ServiceError::Validation(format!(
            "at most {MAX_REACTIONS_PER_USER} reactions per user on one message"
        )));
    }
    Ok(())
}

#[derive(Clone)]
pub struct DefaultMessageReactionService {
    repos: ReactionRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    /// Tells a message's author about a reaction, best-effort. `None` in unit tests that are not
    /// about notifications; the composition root wires it.
    notification_service: Option<Arc<dyn NotificationService>>,
    /// Whether custom emoji, and so `:name:` reactions, are enabled. `None` refuses `:name:`
    /// reactions.
    features: Option<Arc<dyn FeatureGate>>,
    /// The request inbox: a recipient reacts in no conversation they have not accepted. `None`
    /// in unit tests that are not about it; the composition root wires it.
    relay_repo: Option<Arc<dyn RelayRepository>>,
}

impl DefaultMessageReactionService {
    pub fn new(repos: ReactionRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
            notification_service: None,
            features: None,
            relay_repo: None,
        }
    }

    /// Refuse reactions in a conversation whose message request the reactor has not accepted.
    pub fn with_relay_repo(mut self, relay_repo: Arc<dyn RelayRepository>) -> Self {
        self.relay_repo = Some(relay_repo);
        self
    }

    /// Accept `:name:` reactions while the instance has custom emoji enabled.
    pub fn with_feature_gate(mut self, features: Arc<dyn FeatureGate>) -> Self {
        self.features = Some(features);
        self
    }

    /// `Validation` unless a new `:name:` reaction names a custom emoji of `server_id` while
    /// custom emoji are enabled. Unicode emoji always pass.
    ///
    /// # Errors
    /// `Validation` as above; storage errors reading the custom emoji.
    async fn ensure_shortcode_exists(
        &self,
        repos: &ReactionRepos,
        server_id: ServerId,
        emoji: &str,
    ) -> ServiceResult<()> {
        let Some(name) = emoji
            .strip_prefix(':')
            .and_then(|rest| rest.strip_suffix(':'))
        else {
            return Ok(());
        };
        let enabled = self
            .features
            .as_ref()
            .is_some_and(|gate| gate.state(ServerFeature::CustomEmoji).is_enabled());
        if !enabled {
            return Err(ServiceError::Validation(
                "custom emoji reactions are not available on this server".to_string(),
            ));
        }
        let exists = repos
            .custom_emoji_repo
            .list_by_server(server_id)
            .await?
            .iter()
            .any(|custom| custom.name == name);
        if !exists {
            return Err(ServiceError::Validation(format!(
                "this workspace has no custom emoji :{name}:"
            )));
        }
        Ok(())
    }

    pub fn with_notification_service(
        mut self,
        notification_service: Arc<dyn NotificationService>,
    ) -> Self {
        self.notification_service = Some(notification_service);
        self
    }

    /// Best-effort notification of the author about an added reaction: a failure is logged and
    /// never fails the reaction.
    async fn notify_author(&self, cmd: NotifyForReactionCmd) {
        let Some(notification_service) = &self.notification_service else {
            return;
        };
        let message_id = cmd.message.id;
        if let Err(e) = notification_service.notify_for_reaction(cmd).await {
            tracing::warn!(error = %e, message_id = %message_id, "failed to notify about a reaction");
        }
    }

    async fn begin_write(&self) -> ServiceResult<(ReactionRepos, Option<TransactionScope>)> {
        if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let repos = AppRepos::with_handle(&scope.handle()).for_reaction();
            Ok((repos, Some(scope)))
        } else {
            Ok((self.repos.clone(), None))
        }
    }

    async fn commit_if_present(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn group_reactions(
        reactions: Vec<MessageReaction>,
        actor_user_id: rim_core::user::model::UserId,
    ) -> Vec<ReactionGroupDto> {
        use std::collections::BTreeMap;

        use rim_services::message_reaction::dto::MAX_REACTION_USER_IDS;

        let mut groups: BTreeMap<String, (u32, bool, Vec<rim_core::user::model::UserId>)> =
            BTreeMap::new();
        for r in &reactions {
            let entry = groups
                .entry(r.emoji.clone())
                .or_insert((0, false, Vec::new()));
            entry.0 += 1;
            if r.user_id == actor_user_id {
                entry.1 = true;
            }
            entry.2.push(r.user_id);
        }

        groups
            .into_iter()
            .map(|(emoji, (count, reacted_by_me, user_ids))| {
                let mut user_ids = user_ids;
                user_ids.truncate(MAX_REACTION_USER_IDS);
                ReactionGroupDto {
                    emoji,
                    count,
                    reacted_by_me,
                    user_ids,
                }
            })
            .collect()
    }

    /// The message the actor reacts to, with its conversation and the actor's standing in the
    /// workspace, once the conversation is writable for the actor and the message is live.
    async fn ensure_actor_can_react(
        &self,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        message_id: rim_core::message::model::MessageId,
        actor_user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<(Message, Conversation, MemberStanding)> {
        let (conversation, standing) = ensure_conversation_writable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            actor_user_id,
            server_id,
            conversation_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        if message.is_deleted {
            return Err(ServiceError::Validation(
                "cannot react to a deleted message".to_string(),
            ));
        }

        Ok((message, conversation, standing))
    }
}

#[async_trait]
impl MessageReactionService for DefaultMessageReactionService {
    async fn toggle_reaction(
        &self,
        cmd: ToggleReactionCmd,
    ) -> ServiceResult<Vec<ReactionGroupDto>> {
        ensure_request_decided(
            self.relay_repo.as_ref(),
            cmd.actor.user_id,
            cmd.conversation_id,
        )
        .await?;
        // Only an added reaction must be an emoji; the caller removes any reaction they hold,
        // whatever its text.
        let emoji = trim_reaction(&cmd.emoji)?.to_string();

        let (message, conversation, standing) = self
            .ensure_actor_can_react(
                cmd.server_id,
                cmd.conversation_id,
                cmd.message_id,
                cmd.actor.user_id,
            )
            .await?;

        // Lock the message, read, check the caps and add or remove inside one transaction, so
        // concurrent toggles cannot pass the caps.
        let (repos, scope) = self.begin_write().await?;

        if !repos
            .message_reaction_repo
            .lock_message(cmd.message_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        let mut all = repos
            .message_reaction_repo
            .get_reactions_for_message(cmd.message_id)
            .await?;
        let existing = all
            .iter()
            .position(|r| r.user_id == cmd.actor.user_id && r.emoji == emoji);

        let added = existing.is_none();
        match existing {
            None => {
                ensure_can_post(&conversation, &standing, PostKind::Reaction)?;
                validate_reaction_emoji(&emoji)?;
                ensure_room_for_reaction(&all, cmd.actor.user_id, &emoji)?;
                if !all.iter().any(|r| r.emoji == emoji) {
                    self.ensure_shortcode_exists(&repos, cmd.server_id, &emoji)
                        .await?;
                }
                let reaction = repos
                    .message_reaction_repo
                    .add_reaction(NewMessageReaction {
                        message_id: cmd.message_id,
                        user_id: cmd.actor.user_id,
                        emoji: emoji.clone(),
                    })
                    .await?;
                all.push(reaction);
            }
            Some(index) => {
                repos
                    .message_reaction_repo
                    .remove_reaction(cmd.message_id, cmd.actor.user_id, &emoji)
                    .await?;
                all.remove(index);
            }
        }

        Self::commit_if_present(scope).await?;

        let groups = Self::group_reactions(all, cmd.actor.user_id);

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await?;

        self.observers.emit(WorkspaceEvent::ReactionsUpdated {
            workspace_id: cmd.server_id,
            conversation_id: cmd.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids: member_user_ids.clone(),
            message_id: cmd.message_id,
            reactions: groups
                .iter()
                .map(|g| ReactionGroupData {
                    emoji: g.emoji.clone(),
                    count: g.count,
                    user_ids: g.user_ids.clone(),
                })
                .collect(),
        });

        if added && message.sent_by != cmd.actor.user_id {
            self.notify_author(NotifyForReactionCmd {
                server_id: cmd.server_id,
                conversation: conv,
                message,
                member_user_ids,
                emoji,
                by: cmd.actor.user_id,
            })
            .await;
        }

        Ok(groups)
    }

    async fn get_reactions(
        &self,
        query: GetMessageReactionsQuery,
    ) -> ServiceResult<Vec<ReactionGroupDto>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(query.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }

        let all = self
            .repos
            .message_reaction_repo
            .get_reactions_for_message(query.message_id)
            .await?;

        Ok(Self::group_reactions(all, query.actor.user_id))
    }

    async fn get_reactions_for_messages(
        &self,
        query: GetReactionsForMessagesQuery,
    ) -> ServiceResult<HashMap<MessageId, Vec<ReactionGroupDto>>> {
        if query.message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let raw = self
            .repos
            .message_reaction_repo
            .get_reactions_for_messages(&query.message_ids)
            .await?;

        let grouped = raw
            .into_iter()
            .map(|(msg_id, reactions)| {
                (
                    msg_id,
                    Self::group_reactions(reactions, query.actor.user_id),
                )
            })
            .collect();

        Ok(grouped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_the_picker_offers_are_accepted() {
        for emoji in [
            "\u{1f44d}",                                                       // thumbs up
            "\u{1f44d}\u{1f3fd}",                                              // with a skin tone
            "\u{2764}\u{fe0f}",   // red heart, emoji presentation
            "\u{263a}\u{fe0f}",   // smiling face
            "\u{00a9}\u{fe0f}",   // copyright sign
            "#\u{fe0f}\u{20e3}",  // keycap number sign
            "1\u{fe0f}\u{20e3}",  // keycap one
            "\u{1f1e8}\u{1f1ff}", // flag: Czechia
            "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}", // flag: Scotland
            "\u{1f469}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}", // family
            "\u{1f9d1}\u{1f3fb}\u{200d}\u{1f9b0}", // person with red hair
            "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}", // rainbow flag
            "\u{1faf8}\u{1f3ff}", // rightwards pushing hand
            ":party_parrot:",
            ":čau1:",
        ] {
            assert_eq!(
                validate_reaction_emoji(&format!(" {emoji} ")).expect(emoji),
                emoji
            );
        }
    }

    #[test]
    fn text_digits_and_malformed_shortcodes_are_refused() {
        for text in [
            "",
            "   ",
            "verify at evil.co",
            "a",
            "123",
            "#",
            "*",
            "\u{fe0f}",
            "\u{200d}",
            "\u{1f44d} ok",
            "::",
            ":a b:",
            ":evil.co:",
            "a:b:",
            "<b>\u{1f44d}</b>",
        ] {
            assert!(validate_reaction_emoji(text).is_err(), "{text:?} passed");
        }
        assert!(validate_reaction_emoji(&format!(":{}:", "a".repeat(31))).is_err());
        assert!(validate_reaction_emoji(&format!(":{}:", "a".repeat(30))).is_ok());
    }
}
