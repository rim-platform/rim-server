use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::instrument;

use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::model::MessageId;
use rim_core::message::repository::MessageRepository;
use rim_core::notification::model::{
    DndPause, DndWindow, KEYWORD_LEN, MAX_KEYWORDS, NewReactionNotification, NewUserNotification,
    NotificationLevel, NotificationReaction, UserNotificationSettings,
};
use rim_core::notification::repository::{ListUserNotificationsParams, NotificationRepository};
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::notification::command::{
    GetConversationNotificationLevelQuery, GetNotificationSettingsQuery, ListNotificationsQuery,
    MarkNotificationsReadCmd, MarkNotificationsReadTarget, NotifyForMessageCmd,
    NotifyForReactionCmd, SetConversationNotificationLevelCmd, SetDndPauseCmd,
    UnreadNotificationCountQuery, UpdateNotificationSettingsCmd,
};
use rim_services::notification::dto::{
    ConversationNotificationLevelDto, NotificationDto, NotificationPageCursor,
    NotificationSettingsDto,
};
use rim_services::notification::service::NotificationService;
use rim_services::{ServiceError, ServiceResult};

use super::decide::{DecisionInput, ReactionDecisionInput, decide, decide_reaction};
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_server_member,
};

/// Largest page of `list_notifications`.
const MAX_PAGE_LIMIT: u32 = 100;
/// Most ids one `mark_read` call accepts.
const MAX_MARK_READ_IDS: usize = 500;

#[derive(Clone)]
pub struct NotificationRepos {
    pub notification_repo: Arc<dyn NotificationRepository>,
    pub notification_preference_repo: Arc<dyn NotificationPreferenceRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Thread participants for thread-reply notifications.
    pub message_repo: Arc<dyn MessageRepository>,
}

pub struct DefaultNotificationService {
    repos: NotificationRepos,
    observers: Observers,
}

impl DefaultNotificationService {
    pub fn new(repos: NotificationRepos, observers: Observers) -> Self {
        Self { repos, observers }
    }

    async fn load_settings(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> ServiceResult<UserNotificationSettings> {
        Ok(self
            .repos
            .notification_repo
            .get_settings(user_id, server_id)
            .await?
            .unwrap_or_else(|| UserNotificationSettings::defaults(user_id, server_id, Utc::now())))
    }

    async fn level_dto(
        &self,
        user_id: UserId,
        server_id: ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        pref: Option<NotificationPreference>,
    ) -> ServiceResult<ConversationNotificationLevelDto> {
        let settings = self.load_settings(user_id, server_id).await?;
        Ok(level_dto(
            conversation_id,
            pref.as_ref(),
            settings.default_level,
            Utc::now(),
        ))
    }
}

/// The level view of one conversation: a timed mute in effect reads as `NONE`.
fn level_dto(
    conversation_id: rim_core::conversation::model::ConversationId,
    pref: Option<&NotificationPreference>,
    default_level: NotificationLevel,
    now: chrono::DateTime<Utc>,
) -> ConversationNotificationLevelDto {
    let level = pref.and_then(|p| p.level);
    let effective_level = if pref.is_some_and(|p| p.is_timed_muted_at(now)) {
        NotificationLevel::None
    } else {
        level.unwrap_or(default_level)
    };
    ConversationNotificationLevelDto {
        conversation_id,
        level,
        effective_level,
        muted_until: pref.and_then(|p| p.muted_until),
    }
}

/// Trim, validate (2..=40 characters, no control characters, at most 20 distinct), lower-case
/// and deduplicate keywords, keeping their first-seen order.
pub(crate) fn normalize_keywords(keywords: &[String]) -> ServiceResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in keywords {
        let keyword = raw.trim().to_lowercase();
        let len = keyword.chars().count();
        if !KEYWORD_LEN.contains(&len) {
            return Err(ServiceError::Validation(format!(
                "keywords must be {} to {} characters long",
                KEYWORD_LEN.start(),
                KEYWORD_LEN.end()
            )));
        }
        if keyword.chars().any(char::is_control) {
            return Err(ServiceError::Validation(
                "keywords must not contain control characters".to_string(),
            ));
        }
        if !out.contains(&keyword) {
            out.push(keyword);
        }
    }
    if out.len() > MAX_KEYWORDS {
        return Err(ServiceError::Validation(format!(
            "at most {MAX_KEYWORDS} keywords are allowed"
        )));
    }
    Ok(out)
}

/// The window's timezone must be an IANA name and the window must not be empty.
pub(crate) fn validate_dnd_window(window: &DndWindow) -> ServiceResult<()> {
    let timezone = window.timezone.trim();
    if chrono_tz::Tz::from_str(timezone).is_err() {
        return Err(ServiceError::Validation(format!(
            "unknown timezone `{timezone}` (expected an IANA name such as Europe/Prague)"
        )));
    }
    if window.start == window.end {
        return Err(ServiceError::Validation(
            "do-not-disturb start and end must differ".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
impl NotificationService for DefaultNotificationService {
    async fn get_settings(
        &self,
        query: GetNotificationSettingsQuery,
    ) -> ServiceResult<NotificationSettingsDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        Ok(self
            .load_settings(query.actor.user_id, query.server_id)
            .await?
            .into())
    }

    #[instrument(name = "notification.update_settings", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id))]
    async fn update_settings(
        &self,
        cmd: UpdateNotificationSettingsCmd,
    ) -> ServiceResult<NotificationSettingsDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        let keywords = normalize_keywords(&cmd.keywords)?;
        let dnd = match cmd.dnd {
            Some(window) => {
                validate_dnd_window(&window)?;
                Some(DndWindow {
                    timezone: window.timezone.trim().to_string(),
                    ..window
                })
            }
            None => None,
        };
        let current = self.load_settings(cmd.actor.user_id, cmd.server_id).await?;
        let saved = self
            .repos
            .notification_repo
            .upsert_settings(UserNotificationSettings {
                default_level: cmd.default_level,
                keywords,
                dnd,
                digest: cmd.digest,
                reactions: cmd.reactions.unwrap_or(current.reactions),
                updated_at: Utc::now(),
                ..current
            })
            .await?;
        Ok(saved.into())
    }

    #[instrument(name = "notification.set_dnd_pause", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id))]
    async fn set_dnd_pause(&self, cmd: SetDndPauseCmd) -> ServiceResult<NotificationSettingsDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        let now = Utc::now();
        if let DndPause::Until(until) = cmd.pause
            && until <= now
        {
            return Err(ServiceError::Validation(
                "do-not-disturb `until` must be in the future".to_string(),
            ));
        }
        let current = self.load_settings(cmd.actor.user_id, cmd.server_id).await?;
        let saved = self
            .repos
            .notification_repo
            .upsert_settings(UserNotificationSettings {
                dnd_pause: cmd.pause,
                updated_at: now,
                ..current
            })
            .await?;
        Ok(saved.into())
    }

    #[instrument(name = "notification.set_conversation_level", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id))]
    async fn set_conversation_level(
        &self,
        cmd: SetConversationNotificationLevelCmd,
    ) -> ServiceResult<ConversationNotificationLevelDto> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        let pref = self
            .repos
            .notification_preference_repo
            .set_level(cmd.actor.user_id, cmd.conversation_id, cmd.level)
            .await?;
        self.level_dto(cmd.actor.user_id, cmd.server_id, cmd.conversation_id, pref)
            .await
    }

    async fn get_conversation_level(
        &self,
        query: GetConversationNotificationLevelQuery,
    ) -> ServiceResult<ConversationNotificationLevelDto> {
        let pref = self
            .repos
            .notification_preference_repo
            .get_preference(query.actor.user_id, query.conversation_id)
            .await?;
        self.level_dto(
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
            pref,
        )
        .await
    }

    async fn list_notifications(
        &self,
        query: ListNotificationsQuery,
    ) -> ServiceResult<Page<NotificationDto, NotificationPageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        let page = self
            .repos
            .notification_repo
            .list_notifications(ListUserNotificationsParams {
                user_id: query.actor.user_id,
                server_id: query.server_id,
                unread_only: query.unread_only,
                limit: query.limit.clamp(1, MAX_PAGE_LIMIT),
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
            })
            .await?;
        Ok(Page::new(
            page.items.into_iter().map(Into::into).collect(),
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn unread_count(&self, query: UnreadNotificationCountQuery) -> ServiceResult<u64> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        Ok(self
            .repos
            .notification_repo
            .unread_count(query.actor.user_id, query.server_id)
            .await?)
    }

    #[instrument(name = "notification.mark_read", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id))]
    async fn mark_read(&self, cmd: MarkNotificationsReadCmd) -> ServiceResult<u64> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        let now = Utc::now();
        let changed = match cmd.target {
            MarkNotificationsReadTarget::Ids(ids) => {
                if ids.len() > MAX_MARK_READ_IDS {
                    return Err(ServiceError::Validation(format!(
                        "at most {MAX_MARK_READ_IDS} notification ids per call"
                    )));
                }
                self.repos
                    .notification_repo
                    .mark_read_by_ids(cmd.actor.user_id, cmd.server_id, &ids, now)
                    .await?
            }
            MarkNotificationsReadTarget::AllBefore(before) => {
                self.repos
                    .notification_repo
                    .mark_read_before(cmd.actor.user_id, cmd.server_id, before.min(now), now)
                    .await?
            }
        };
        Ok(changed)
    }

    #[instrument(name = "notification.notify_for_message", skip_all, fields(server_id = %cmd.server_id, conversation_id = %cmd.conversation.id, message_id = %cmd.message.id))]
    async fn notify_for_message(&self, cmd: NotifyForMessageCmd) -> ServiceResult<usize> {
        let author = cmd.message.sent_by;
        let candidates: Vec<UserId> = cmd
            .member_user_ids
            .iter()
            .copied()
            .filter(|id| *id != author)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if candidates.is_empty() || cmd.message.is_deleted {
            return Ok(0);
        }

        let thread_participants: HashSet<UserId> = match cmd.message.thread_id {
            Some(thread_id) => self
                .repos
                .message_repo
                .list_thread_participant_ids(thread_id)
                .await?
                .into_iter()
                .collect(),
            None => HashSet::new(),
        };
        let prefs_by_user: HashMap<UserId, NotificationPreference> = self
            .repos
            .notification_preference_repo
            .list_for_conversation(cmd.conversation.id, &candidates)
            .await?
            .into_iter()
            .map(|p| (p.user_id, p))
            .collect();
        let settings_by_user: HashMap<UserId, UserNotificationSettings> = self
            .repos
            .notification_repo
            .get_settings_for_users(cmd.server_id, &candidates)
            .await?
            .into_iter()
            .map(|s| (s.user_id, s))
            .collect();

        let now = Utc::now();
        let decisions = decide(&DecisionInput {
            message: &cmd.message,
            conversation: &cmd.conversation,
            thread_participants: &thread_participants,
            members: &cmd.member_user_ids,
            prefs_by_user: &prefs_by_user,
            settings_by_user: &settings_by_user,
            now,
            encrypted_mentions: cmd.encrypted_mentions.as_ref(),
        });
        if decisions.is_empty() {
            return Ok(0);
        }

        let new_rows: Vec<NewUserNotification> = decisions
            .iter()
            .map(|d| NewUserNotification {
                user_id: d.user_id,
                server_id: cmd.server_id,
                conversation_id: cmd.conversation.id,
                message_id: cmd.message.id,
                reason: d.reason,
            })
            .collect();
        let inserted = self
            .repos
            .notification_repo
            .create_notifications(&new_rows, now)
            .await?;

        let quiet_by_user: HashMap<UserId, bool> =
            decisions.iter().map(|d| (d.user_id, d.quiet)).collect();
        for row in &inserted {
            self.observers.emit(WorkspaceEvent::NotificationDue {
                workspace_id: row.server_id,
                user_id: row.user_id,
                notification_id: row.id,
                conversation_id: row.conversation_id,
                message_id: row.message_id,
                reason: row.reason.as_str().to_string(),
                quiet: quiet_by_user.get(&row.user_id).copied().unwrap_or(false),
                reaction_emoji: None,
                reaction_by: None,
                member_user_ids: vec![row.user_id],
            });
        }
        Ok(inserted.len())
    }

    #[instrument(name = "notification.notify_for_reaction", skip_all, fields(server_id = %cmd.server_id, conversation_id = %cmd.conversation.id, message_id = %cmd.message.id))]
    async fn notify_for_reaction(&self, cmd: NotifyForReactionCmd) -> ServiceResult<bool> {
        let author = cmd.message.sent_by;
        // Skips the loads when no rule could pass; `decide_reaction` applies the same checks.
        if author == cmd.by || cmd.message.is_deleted || !cmd.member_user_ids.contains(&author) {
            return Ok(false);
        }
        let pref = self
            .repos
            .notification_preference_repo
            .get_preference(author, cmd.conversation.id)
            .await?;
        let settings = self
            .repos
            .notification_repo
            .get_settings(author, cmd.server_id)
            .await?;
        let now = Utc::now();
        let Some(decision) = decide_reaction(&ReactionDecisionInput {
            message: &cmd.message,
            by: cmd.by,
            members: &cmd.member_user_ids,
            pref: pref.as_ref(),
            settings: settings.as_ref(),
            now,
        }) else {
            return Ok(false);
        };

        let Some(row) = self
            .repos
            .notification_repo
            .upsert_reaction_notification(
                NewReactionNotification {
                    user_id: author,
                    server_id: cmd.server_id,
                    conversation_id: cmd.conversation.id,
                    message_id: cmd.message.id,
                    reaction: NotificationReaction {
                        emoji: cmd.emoji,
                        by: cmd.by,
                    },
                },
                now,
            )
            .await?
        else {
            return Ok(false);
        };
        let reaction = row.reaction.clone();
        self.observers.emit(WorkspaceEvent::NotificationDue {
            workspace_id: row.server_id,
            user_id: row.user_id,
            notification_id: row.id,
            conversation_id: row.conversation_id,
            message_id: row.message_id,
            reason: row.reason.as_str().to_string(),
            quiet: decision.quiet,
            reaction_emoji: reaction.as_ref().map(|r| r.emoji.clone()),
            reaction_by: reaction.map(|r| r.by),
            member_user_ids: vec![row.user_id],
        });
        Ok(true)
    }

    async fn forget_message(&self, message_id: MessageId) -> ServiceResult<u64> {
        Ok(self
            .repos
            .notification_repo
            .delete_unread_for_message(message_id)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_trimmed_lowercased_and_deduplicated() {
        let out = normalize_keywords(&[
            " Deploy ".to_string(),
            "deploy".to_string(),
            "On-Call".to_string(),
        ])
        .expect("valid");
        assert_eq!(out, vec!["deploy".to_string(), "on-call".to_string()]);
    }

    #[test]
    fn keyword_length_and_count_limits() {
        assert!(normalize_keywords(&["a".to_string()]).is_err());
        assert!(normalize_keywords(&["  b  ".to_string()]).is_err());
        assert!(normalize_keywords(&["x".repeat(41)]).is_err());
        assert!(normalize_keywords(&["x".repeat(40)]).is_ok());
        assert!(normalize_keywords(&["ab".to_string()]).is_ok());
        assert!(normalize_keywords(&["tab\tbed".to_string()]).is_err());
        let twenty: Vec<String> = (0..20).map(|i| format!("kw{i}")).collect();
        assert!(normalize_keywords(&twenty).is_ok());
        let twenty_one: Vec<String> = (0..21).map(|i| format!("kw{i}")).collect();
        assert!(normalize_keywords(&twenty_one).is_err());
        // Duplicates do not count twice.
        let mut dupes = twenty.clone();
        dupes.push("KW0".to_string());
        assert_eq!(normalize_keywords(&dupes).expect("valid").len(), 20);
        // Length counts characters, not bytes.
        assert!(normalize_keywords(&["é".repeat(40)]).is_ok());
    }

    #[test]
    fn dnd_window_validation() {
        let window = |tz: &str, start: u32, end: u32| DndWindow {
            start: chrono::NaiveTime::from_hms_opt(start, 0, 0).expect("time"),
            end: chrono::NaiveTime::from_hms_opt(end, 0, 0).expect("time"),
            timezone: tz.to_string(),
        };
        assert!(validate_dnd_window(&window("Europe/Prague", 22, 7)).is_ok());
        assert!(validate_dnd_window(&window("UTC", 9, 17)).is_ok());
        assert!(validate_dnd_window(&window("Mars/Olympus", 22, 7)).is_err());
        assert!(validate_dnd_window(&window("UTC", 8, 8)).is_err());
    }

    #[test]
    fn effective_level_reflects_override_default_and_timed_mute() {
        let now = Utc::now();
        let conv = rim_core::conversation::model::ConversationId::new();
        let mut pref = NotificationPreference {
            user_id: UserId::new(),
            conversation_id: conv,
            muted_until: None,
            level: None,
        };
        let dto = level_dto(conv, None, NotificationLevel::Mentions, now);
        assert_eq!(dto.level, None);
        assert_eq!(dto.effective_level, NotificationLevel::Mentions);

        pref.level = Some(NotificationLevel::All);
        let dto = level_dto(conv, Some(&pref), NotificationLevel::None, now);
        assert_eq!(dto.level, Some(NotificationLevel::All));
        assert_eq!(dto.effective_level, NotificationLevel::All);

        pref.muted_until = Some(now + chrono::Duration::minutes(5));
        let dto = level_dto(conv, Some(&pref), NotificationLevel::All, now);
        assert_eq!(dto.effective_level, NotificationLevel::None);
        assert_eq!(dto.level, Some(NotificationLevel::All));

        pref.muted_until = Some(now - chrono::Duration::minutes(5));
        let dto = level_dto(conv, Some(&pref), NotificationLevel::All, now);
        assert_eq!(dto.effective_level, NotificationLevel::All);
    }
}
