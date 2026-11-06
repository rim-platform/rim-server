//! Bot accounts of a workspace and their tokens: who may manage them, the limits, and whether a
//! presented token works. The incoming webhooks bound to a bot are in [`webhook`].

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use tracing::{instrument, warn};

use rim_auth::session_token::{generate_bot_token, hash_token, is_bot_token_shape};
use rim_core::bot::model::{
    Bot, BotPatch, BotTokenId, IncomingWebhookId, NewBot, NewBotToken, WebhookPreset,
};
use rim_core::bot::repository::BotRepository;
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message_mention::repository::MessageMentionRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::server::model::ServerId;
use rim_core::server_feature::model::ServerFeature;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::bot::dto::{
    BotActorDto, BotDto, BotInfoDto, BotMentionCursor, BotMentionsPage, BotMessageDto, BotTokenDto,
    CreatedBotDto, CreatedBotTokenDto, CreatedIncomingWebhookDto, IncomingWebhookDto,
};
use rim_services::bot::service::BotService;
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;
use rim_services::conversation::service::ConversationService;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::feature::service::FeatureGate;
use rim_services::limits::InstanceLimits;
use rim_services::moderation::command::KickMemberCmd;
use rim_services::moderation::service::ModerationService;
use rim_services::{Actor, ServiceError, ServiceResult};
use sea_orm::DatabaseConnection;

use crate::display_name;
use crate::moderation::{ModerationAuditEntry, record_audit_action};
use crate::repos::AppRepos;
use crate::reserved_names::ReservedNames;
use crate::signup::normalize_username;
use crate::util::{add_workspace_member, ensure_workspace_features};

mod webhook;
mod webhook_text;

/// The most bots a workspace holds that are not deactivated.
const MAX_ACTIVE_BOTS: u64 = 25;
/// The most tokens a bot holds that are not revoked.
const MAX_ACTIVE_TOKENS: u64 = 5;
/// The most characters of `run_by` and of `reads_go_to`.
const MAX_DISCLOSURE_CHARS: usize = 200;
/// The most characters of a token's label.
const MAX_TOKEN_LABEL_CHARS: usize = 64;
/// The label of the token a bot is created with.
const FIRST_TOKEN_LABEL: &str = "default";
/// How long a token's last use stands before a newer one is recorded, in seconds.
const LAST_USED_RESOLUTION_SECS: i64 = 60;
/// The most characters of the name a bot's username keeps.
const MAX_USERNAME_NAME_CHARS: usize = 16;
/// The hex digits of a bot's username that keep it unique.
const USERNAME_SUFFIX_CHARS: usize = 8;

const AUDIT_CREATE: &str = "bot.create";
const AUDIT_UPDATE: &str = "bot.update";
const AUDIT_DEACTIVATE: &str = "bot.deactivate";
const AUDIT_TOKEN_CREATE: &str = "bot.token.create";
const AUDIT_TOKEN_REVOKE: &str = "bot.token.revoke";
const AUDIT_MENTIONS: &str = "bot.mentions";

/// How far back a bot's first look at its mentions goes, in hours.
const FIRST_MENTIONS_WINDOW_HOURS: i64 = 24;

/// The repositories a bot is managed with. Inside a transaction, built from its handle.
#[derive(Clone)]
pub struct BotRepos {
    pub bot_repo: Arc<dyn BotRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Consulted when a member joins, which a guest skips.
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// A banned bot's tokens stop working.
    pub moderation_repo: Arc<dyn ModerationRepository>,
    /// Where a bot reads its mentions from.
    pub message_mention_repo: Arc<dyn MessageMentionRepository>,
}

pub struct DefaultBotService {
    repos: BotRepos,
    observers: Observers,
    feature_gate: Arc<dyn FeatureGate>,
    /// Takes a deactivated bot out of its workspace, as a kick takes anyone.
    moderation: Arc<dyn ModerationService>,
    /// Adds a webhook's bot to its channel, under the rules of adding anyone.
    conversations: Arc<dyn ConversationService>,
    audit_log_service: Option<Arc<dyn AuditLogService>>,
    direct_space_id: Option<ServerId>,
    reserved_names: ReservedNames,
    limits: InstanceLimits,
    /// Runs each creation in one transaction; without it, unit tests use the service's own repos.
    db: Option<DatabaseConnection>,
}

impl DefaultBotService {
    pub fn new(
        repos: BotRepos,
        observers: Observers,
        feature_gate: Arc<dyn FeatureGate>,
        moderation: Arc<dyn ModerationService>,
        conversations: Arc<dyn ConversationService>,
    ) -> Self {
        Self {
            repos,
            observers,
            feature_gate,
            moderation,
            conversations,
            audit_log_service: None,
            direct_space_id: None,
            reserved_names: ReservedNames::default(),
            limits: InstanceLimits::default(),
            db: None,
        }
    }

    pub fn with_audit_log_service(mut self, audit_log_service: Arc<dyn AuditLogService>) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    /// The relay's direct space, which has no bots, or `None` where none is wired.
    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    /// The names a bot may not take.
    pub fn with_reserved_names(mut self, reserved_names: ReservedNames) -> Self {
        self.reserved_names = reserved_names;
        self
    }

    /// The instance's cap on members per workspace, which a bot counts against.
    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_database(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    async fn transaction(&self) -> ServiceResult<(Option<TransactionScope>, BotRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_bot(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn require_feature(&self) -> ServiceResult<()> {
        self.feature_gate.require(ServerFeature::Bots)
    }

    /// `PermissionDenied` unless `actor` is an Owner or Moderator of `server_id`.
    async fn ensure_moderator(&self, server_id: ServerId, actor: UserId) -> ServiceResult<()> {
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if !member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    /// The bot `user_id`, after the checks of every call about one bot: the feature is on, the bot
    /// exists and `actor` is an Owner or Moderator of its workspace.
    async fn managed_bot(&self, actor: UserId, user_id: UserId) -> ServiceResult<Bot> {
        self.require_feature()?;
        let bot = self
            .repos
            .bot_repo
            .find_bot(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.ensure_moderator(bot.server_id, actor).await?;
        Ok(bot)
    }

    /// Whether `conversation_id` is a plaintext channel of `server_id` with the bot `user_id` a
    /// member of it.
    async fn bot_is_in_channel(
        &self,
        server_id: ServerId,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> ServiceResult<bool> {
        let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(conversation_id)
            .await?
        else {
            return Ok(false);
        };
        if conversation.server_id != server_id
            || conversation.conversation_type != ConversationType::Channel
            || conversation.is_encrypted()
        {
            return Ok(false);
        }
        Ok(self
            .repos
            .conversation_member_repo
            .is_member(conversation_id, user_id)
            .await?)
    }

    async fn audit(
        &self,
        actor: UserId,
        server_id: ServerId,
        command_id: &'static str,
        args: serde_json::Value,
        typed_verb: TypedVerb,
    ) {
        record_audit_action(
            self.audit_log_service.as_ref(),
            ModerationAuditEntry {
                actor,
                server_id,
                command_id,
                args,
                dangerous: typed_verb == TypedVerb::Delete,
                typed_verb,
            },
            CommandInitiator::Palette.as_audit_str(),
        )
        .await;
    }
}

/// `field` trimmed, when it is 1 to `max_chars` characters with no control or format character,
/// which could hide or reorder the text around it.
///
/// # Errors
/// `Validation` naming `what` and the rule that failed.
fn plain_text(field: &str, what: &str, max_chars: usize) -> ServiceResult<String> {
    let trimmed = field.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_chars {
        return Err(ServiceError::Validation(format!(
            "{what} must be between 1 and {max_chars} characters"
        )));
    }
    if trimmed.chars().any(display_name::is_control_or_format) {
        return Err(ServiceError::Validation(format!(
            "{what} must not contain control characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// A bot's name in the form every display name is stored in, when it leaves anything to show and
/// the instance does not reserve it.
///
/// # Errors
/// `Validation` when it is empty, too long or reserved.
fn bot_name(name: &str, reserved: &ReservedNames) -> ServiceResult<String> {
    let cleaned = display_name::clean_local(name)?;
    if cleaned.is_empty() {
        return Err(ServiceError::Validation("a bot needs a name".to_string()));
    }
    reserved.check(&cleaned)?;
    Ok(cleaned)
}

/// A username for a bot named `name`: `bot-`, what ASCII the name holds and a random suffix, so
/// the prefixes kept for remote and purged accounts never apply.
///
/// # Errors
/// `Validation` when the result breaks the username rules, which no name can make it do.
fn bot_username(name: &str) -> ServiceResult<String> {
    let kept: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(MAX_USERNAME_NAME_CHARS)
        .collect();
    let kept = kept.trim_matches('-');
    let random = uuid::Uuid::new_v4().simple().to_string();
    let suffix = &random[..USERNAME_SUFFIX_CHARS];
    let username = if kept.is_empty() {
        format!("bot-{suffix}")
    } else {
        format!("bot-{kept}-{suffix}")
    };
    normalize_username(&username)
}

/// Whether a token last used at `last_used` should have a use at `now` recorded.
fn last_use_is_stale(last_used: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_used.is_none_or(|at| now - at > Duration::seconds(LAST_USED_RESOLUTION_SECS))
}

#[async_trait]
impl BotService for DefaultBotService {
    #[instrument(name = "bot.create", skip_all, fields(server_id = %server_id))]
    async fn create_bot(
        &self,
        actor: Actor,
        server_id: ServerId,
        name: String,
        run_by: String,
        reads_go_to: String,
    ) -> ServiceResult<CreatedBotDto> {
        self.require_feature()?;
        ensure_workspace_features(self.direct_space_id, server_id)?;
        self.ensure_moderator(server_id, actor.user_id).await?;
        let name = bot_name(&name, &self.reserved_names)?;
        let run_by = plain_text(&run_by, "runBy", MAX_DISCLOSURE_CHARS)?;
        let reads_go_to = plain_text(&reads_go_to, "readsGoTo", MAX_DISCLOSURE_CHARS)?;
        let username = bot_username(&name)?;
        let token = generate_bot_token();

        let (scope, repos) = self.transaction().await?;
        repos.bot_repo.lock_server_bots(server_id).await?;
        if repos.bot_repo.count_active(server_id).await? >= MAX_ACTIVE_BOTS {
            return Err(ServiceError::Validation(format!(
                "a workspace has at most {MAX_ACTIVE_BOTS} active bots"
            )));
        }
        let bot = repos
            .bot_repo
            .create_bot(NewBot {
                server_id,
                username: username.clone(),
                name,
                run_by,
                reads_go_to,
                created_by: actor.user_id,
            })
            .await?;
        add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            server_id,
            bot.user_id,
            ServerMemberRole::Guest,
            self.limits.members_per_workspace,
        )
        .await?;
        let first_token = repos
            .bot_repo
            .create_token(NewBotToken {
                bot_user_id: bot.user_id,
                label: FIRST_TOKEN_LABEL.to_string(),
                token_hash: hash_token(&token),
                created_by: actor.user_id,
            })
            .await?;
        Self::commit(scope).await?;

        self.observers.emit(WorkspaceEvent::MemberJoined {
            workspace_id: server_id,
            user_id: bot.user_id,
            username,
            source: None,
        });
        self.audit(
            actor.user_id,
            server_id,
            AUDIT_CREATE,
            serde_json::json!({
                "bot_user_id": bot.user_id.to_string(),
                "token_id": first_token.id.to_string(),
                "label": first_token.label,
            }),
            TypedVerb::Write,
        )
        .await;
        Ok(CreatedBotDto {
            bot: bot.into(),
            token,
        })
    }

    #[instrument(name = "bot.update", skip_all, fields(bot_user_id = %user_id))]
    async fn update_bot(
        &self,
        actor: Actor,
        user_id: UserId,
        name: Option<String>,
        run_by: Option<String>,
        reads_go_to: Option<String>,
    ) -> ServiceResult<BotDto> {
        let bot = self.managed_bot(actor.user_id, user_id).await?;
        let patch = BotPatch {
            name: name
                .map(|name| bot_name(&name, &self.reserved_names))
                .transpose()?,
            run_by: run_by
                .map(|field| plain_text(&field, "runBy", MAX_DISCLOSURE_CHARS))
                .transpose()?,
            reads_go_to: reads_go_to
                .map(|field| plain_text(&field, "readsGoTo", MAX_DISCLOSURE_CHARS))
                .transpose()?,
        };
        let renamed = patch.name.is_some();
        let updated = self.repos.bot_repo.update_bot(user_id, patch).await?;
        if renamed {
            self.observers.emit(WorkspaceEvent::MemberProfileUpdated {
                workspace_id: bot.server_id,
                user_id,
            });
        }
        self.audit(
            actor.user_id,
            bot.server_id,
            AUDIT_UPDATE,
            serde_json::json!({ "bot_user_id": user_id.to_string() }),
            TypedVerb::Write,
        )
        .await;
        Ok(updated.into())
    }

    #[instrument(name = "bot.deactivate", skip_all, fields(bot_user_id = %user_id))]
    async fn deactivate_bot(&self, actor: Actor, user_id: UserId) -> ServiceResult<BotDto> {
        let bot = self.managed_bot(actor.user_id, user_id).await?;

        let now = Utc::now();
        let (scope, repos) = self.transaction().await?;
        repos.bot_repo.lock_bot(user_id).await?;
        let changed = repos.bot_repo.set_deactivated(user_id, now).await?;
        repos.bot_repo.revoke_all_tokens(user_id, now).await?;
        Self::commit(scope).await?;

        // After the commit, so its tokens are dead whatever happens here; calling again retries.
        if self
            .repos
            .server_member_repo
            .is_member(bot.server_id, user_id)
            .await?
        {
            match self
                .moderation
                .kick(KickMemberCmd {
                    actor,
                    server_id: bot.server_id,
                    user_id,
                    reason: None,
                })
                .await
            {
                Ok(()) | Err(ServiceError::NotFound) => {}
                Err(e) => return Err(e),
            }
        }

        if changed {
            self.audit(
                actor.user_id,
                bot.server_id,
                AUDIT_DEACTIVATE,
                serde_json::json!({ "bot_user_id": user_id.to_string() }),
                TypedVerb::Delete,
            )
            .await;
        }
        let bot = self
            .repos
            .bot_repo
            .find_bot(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(bot.into())
    }

    #[instrument(name = "bot.token.create", skip_all, fields(bot_user_id = %user_id))]
    async fn create_token(
        &self,
        actor: Actor,
        user_id: UserId,
        label: String,
    ) -> ServiceResult<CreatedBotTokenDto> {
        let bot = self.managed_bot(actor.user_id, user_id).await?;
        let label = plain_text(&label, "label", MAX_TOKEN_LABEL_CHARS)?;
        let token = generate_bot_token();

        let (scope, repos) = self.transaction().await?;
        repos.bot_repo.lock_bot(user_id).await?;
        // Read under the lock: a deactivation that committed first is seen here.
        let current = repos
            .bot_repo
            .find_bot(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if current.deactivated_at.is_some() {
            return Err(ServiceError::Validation(
                "a deactivated bot cannot have a new token".to_string(),
            ));
        }
        if repos.bot_repo.count_active_tokens(user_id).await? >= MAX_ACTIVE_TOKENS {
            return Err(ServiceError::Validation(format!(
                "a bot has at most {MAX_ACTIVE_TOKENS} active tokens"
            )));
        }
        let created = repos
            .bot_repo
            .create_token(NewBotToken {
                bot_user_id: user_id,
                label,
                token_hash: hash_token(&token),
                created_by: actor.user_id,
            })
            .await?;
        Self::commit(scope).await?;

        self.audit(
            actor.user_id,
            bot.server_id,
            AUDIT_TOKEN_CREATE,
            serde_json::json!({
                "bot_user_id": user_id.to_string(),
                "token_id": created.id.to_string(),
                "label": created.label,
            }),
            TypedVerb::Write,
        )
        .await;
        Ok(CreatedBotTokenDto {
            bot_token: created.into(),
            token,
        })
    }

    #[instrument(name = "bot.token.revoke", skip_all, fields(token_id = %token_id))]
    async fn revoke_token(&self, actor: Actor, token_id: BotTokenId) -> ServiceResult<BotTokenDto> {
        self.require_feature()?;
        let token = self
            .repos
            .bot_repo
            .find_token(token_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let bot = self
            .repos
            .bot_repo
            .find_bot(token.bot_user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        // The same answer as an unknown id, so an outsider learns nothing about a token.
        self.ensure_moderator(bot.server_id, actor.user_id)
            .await
            .map_err(|e| match e {
                ServiceError::PermissionDenied => ServiceError::NotFound,
                other => other,
            })?;
        if token.revoked_at.is_some() {
            return Ok(token.into());
        }
        let revoked = self
            .repos
            .bot_repo
            .revoke_token(token_id, Utc::now())
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.audit(
            actor.user_id,
            bot.server_id,
            AUDIT_TOKEN_REVOKE,
            serde_json::json!({
                "bot_user_id": bot.user_id.to_string(),
                "token_id": revoked.id.to_string(),
                "label": revoked.label,
            }),
            TypedVerb::Delete,
        )
        .await;
        Ok(revoked.into())
    }

    async fn list_bots(&self, actor: Actor, server_id: ServerId) -> ServiceResult<Vec<BotDto>> {
        self.require_feature()?;
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor.user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if member.member_role.is_guest() {
            return Err(ServiceError::PermissionDenied);
        }
        let bots = self.repos.bot_repo.list_by_server(server_id).await?;
        Ok(bots.into_iter().map(Into::into).collect())
    }

    async fn list_tokens(&self, actor: Actor, user_id: UserId) -> ServiceResult<Vec<BotTokenDto>> {
        self.managed_bot(actor.user_id, user_id).await?;
        let tokens = self.repos.bot_repo.list_tokens(user_id).await?;
        Ok(tokens.into_iter().map(Into::into).collect())
    }

    async fn bot_infos(&self, user_ids: &[UserId]) -> ServiceResult<Vec<BotInfoDto>> {
        let bots = self.repos.bot_repo.list_by_users(user_ids).await?;
        Ok(bots.into_iter().map(Into::into).collect())
    }

    async fn authenticate_token(&self, raw: &str) -> ServiceResult<Option<BotActorDto>> {
        if !is_bot_token_shape(raw) || self.require_feature().is_err() {
            return Ok(None);
        }
        let Some(holder) = self
            .repos
            .bot_repo
            .find_token_holder(&hash_token(raw))
            .await?
        else {
            return Ok(None);
        };
        if holder.token.revoked_at.is_some() || holder.bot_deactivated_at.is_some() {
            return Ok(None);
        }
        let user_id = holder.token.bot_user_id;
        if !self
            .repos
            .server_member_repo
            .is_member(holder.server_id, user_id)
            .await?
        {
            return Ok(None);
        }
        let now = Utc::now();
        if self
            .repos
            .moderation_repo
            .get_active_ban(holder.server_id, user_id, now)
            .await?
            .is_some()
        {
            return Ok(None);
        }
        if last_use_is_stale(holder.token.last_used_at, now)
            && let Err(e) = self.repos.bot_repo.touch_token(holder.token.id, now).await
        {
            warn!(token_id = %holder.token.id, error = %e, "could not record a bot token's use");
        }
        Ok(Some(BotActorDto {
            user_id,
            server_id: holder.server_id,
            token_id: holder.token.id,
        }))
    }

    async fn bot_of(&self, bot: BotActorDto) -> ServiceResult<BotDto> {
        let found = self
            .repos
            .bot_repo
            .find_bot(bot.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(found.into())
    }

    async fn is_in_channel(
        &self,
        bot: BotActorDto,
        conversation_id: ConversationId,
    ) -> ServiceResult<bool> {
        self.bot_is_in_channel(bot.server_id, bot.user_id, conversation_id)
            .await
    }

    async fn list_mentions(
        &self,
        bot: BotActorDto,
        after: Option<BotMentionCursor>,
        limit: u32,
    ) -> ServiceResult<BotMentionsPage> {
        let since = Utc::now() - Duration::hours(FIRST_MENTIONS_WINDOW_HOURS);
        let rows = self
            .repos
            .message_mention_repo
            .list_since(
                bot.user_id,
                after.map(|cursor| (cursor.created_at, cursor.message_id)),
                since,
                u64::from(limit),
            )
            .await;
        if !rows.as_ref().is_ok_and(|found| found.is_empty()) {
            let args = serde_json::json!({ "limit": limit });
            let error = rows.as_ref().err().map(ToString::to_string);
            self.record_read(bot, AUDIT_MENTIONS, args, error).await;
        }
        let rows = rows?;
        let last = rows.last().map(|(mention, _)| BotMentionCursor {
            created_at: mention.created_at,
            message_id: mention.message_id,
        });
        Ok(BotMentionsPage {
            messages: rows
                .into_iter()
                .map(|(_, message)| BotMessageDto::from(message))
                .collect(),
            last,
        })
    }

    async fn record_read(
        &self,
        bot: BotActorDto,
        command_id: &'static str,
        args: serde_json::Value,
        error: Option<String>,
    ) {
        let outcome = if error.is_some() {
            AuditOutcome::Error
        } else {
            AuditOutcome::Success
        };
        tracing::info!(
            target: "audit",
            event = "command_executed",
            actor_id = %bot.user_id,
            command_id,
            initiator = %CommandInitiator::Bot,
            args = %args,
            result_status = outcome.as_str(),
        );
        let Some(audit_log_service) = &self.audit_log_service else {
            return;
        };
        let entry = AuditLogEntryDto {
            id: uuid::Uuid::now_v7(),
            actor_user_id: bot.user_id.into(),
            server_id: bot.server_id.into(),
            command_id: command_id.to_string(),
            initiator: CommandInitiator::Bot.to_string(),
            args_redacted: args,
            sensitivity: CommandSensitivityDto::Safe.as_str().to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: Some(TypedVerb::Read),
            outcome,
            error_kind: error,
            capability_check_outcome: None,
            confirmation_outcome: None,
            parent_audit_id: None,
            run_id: None,
            created_at: Utc::now(),
        };
        if let Err(e) = audit_log_service.write_entry(entry).await {
            warn!(event = "audit_db_write_failed", command_id, error = %e, "audit DB write failed (tracing line already emitted)");
        }
    }

    async fn create_webhook(
        &self,
        actor: Actor,
        conversation_id: ConversationId,
        bot_user_id: UserId,
        preset: WebhookPreset,
    ) -> ServiceResult<CreatedIncomingWebhookDto> {
        self.create_incoming_webhook(actor, conversation_id, bot_user_id, preset)
            .await
    }

    async fn revoke_webhook(
        &self,
        actor: Actor,
        id: IncomingWebhookId,
    ) -> ServiceResult<IncomingWebhookDto> {
        self.revoke_incoming_webhook(actor, id).await
    }

    async fn list_webhooks(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<Vec<IncomingWebhookDto>> {
        self.list_incoming_webhooks(actor, server_id).await
    }

    async fn resolve_webhook(&self, raw: &str) -> ServiceResult<Option<IncomingWebhookDto>> {
        self.resolve_incoming_webhook(raw).await
    }

    fn webhook_text(
        &self,
        preset: WebhookPreset,
        event: Option<String>,
        body: Vec<u8>,
    ) -> ServiceResult<Option<String>> {
        webhook::text_of(preset, event.as_deref(), &body)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rim_core::bot::model::{BotToken, BotTokenHolder};
    use rim_core::bot::repository::MockBotRepository;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message_mention::repository::MockMessageMentionRepository;
    use rim_core::moderation::model::ServerBan;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoResult};
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_services::audit::dto::{AuditLogEntryDto, AuditLogPage, ListAuditLogQuery};
    use rim_services::conversation::service::MockConversationService;
    use rim_services::events::Broadcaster;
    use rim_services::moderation::service::MockModerationService;

    use crate::feature_gate::InstanceFeatureGate;

    use super::*;

    const GOOD_TOKEN_BODY: &str = "abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE";

    #[derive(Default)]
    struct RecordingAudit(Mutex<Vec<AuditLogEntryDto>>);

    #[async_trait]
    impl AuditLogService for RecordingAudit {
        async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()> {
            self.0.lock().expect("lock").push(entry);
            Ok(())
        }

        async fn list_entries(
            &self,
            _caller: Actor,
            _query: ListAuditLogQuery,
        ) -> ServiceResult<AuditLogPage> {
            Err(ServiceError::NotFound)
        }
    }

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    /// The mocks a test sets up before building the service. A call nothing expected panics, so a
    /// gate that stops a call early shows in what a test leaves out.
    struct Fx {
        bots: MockBotRepository,
        members: MockServerMemberRepository,
        moderation_repo: MockModerationRepository,
        moderation: MockModerationService,
        disabled: Vec<ServerFeature>,
        direct_space: Option<ServerId>,
        audit: Arc<RecordingAudit>,
        events: Arc<RecordingBroadcaster>,
    }

    impl Fx {
        fn new() -> Self {
            Self {
                bots: MockBotRepository::new(),
                members: MockServerMemberRepository::new(),
                moderation_repo: MockModerationRepository::new(),
                moderation: MockModerationService::new(),
                disabled: Vec::new(),
                direct_space: None,
                audit: Arc::default(),
                events: Arc::default(),
            }
        }

        /// `get_member` answers with `role` for anyone; `None` makes them no member.
        fn with_role(mut self, role: Option<ServerMemberRole>) -> Self {
            self.members
                .expect_get_member()
                .returning(move |server_id, user_id| {
                    Ok(role.map(|member_role| ServerMember {
                        id: ServerMemberId::new(),
                        server_id,
                        user_id,
                        member_role,
                        joined_at: Utc::now(),
                    }))
                });
            self
        }

        fn feature_off(mut self) -> Self {
            self.disabled.push(ServerFeature::Bots);
            self
        }

        fn audit_handle(&self) -> Arc<RecordingAudit> {
            Arc::clone(&self.audit)
        }

        fn events_handle(&self) -> Arc<RecordingBroadcaster> {
            Arc::clone(&self.events)
        }

        /// The service, with mocks for the conversation repositories that panic when a channel is
        /// looked at or joined.
        fn build(self) -> DefaultBotService {
            let repos = BotRepos {
                bot_repo: Arc::new(self.bots),
                server_member_repo: Arc::new(self.members),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                moderation_repo: Arc::new(self.moderation_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
            };
            DefaultBotService::new(
                repos,
                Observers::new(self.events),
                Arc::new(InstanceFeatureGate::new(&self.disabled, &[])),
                Arc::new(self.moderation),
                Arc::new(MockConversationService::new()),
            )
            .with_audit_log_service(self.audit)
            .with_direct_space(self.direct_space)
            .with_reserved_names(ReservedNames::parse("rim-admin").expect("reserved names"))
        }
    }

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    fn bot_of(server_id: ServerId, user_id: UserId) -> Bot {
        Bot {
            user_id,
            server_id,
            name: "Helper".to_string(),
            run_by: "Ada".to_string(),
            reads_go_to: "Nowhere".to_string(),
            created_by: UserId::new(),
            added_by_name: Some("Ada".to_string()),
            created_at: Utc::now(),
            deactivated_at: None,
        }
    }

    fn token_of(bot_user_id: UserId) -> BotToken {
        BotToken {
            id: BotTokenId::new(),
            bot_user_id,
            label: "default".to_string(),
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        }
    }

    fn ok<T: Send + 'static>(
        value: T,
    ) -> std::pin::Pin<Box<dyn Future<Output = RepoResult<T>> + Send>> {
        Box::pin(async move { Ok(value) })
    }

    /// `bots` finds a bot `user_id` in `server_id`.
    fn expect_bot(bots: &mut MockBotRepository, server_id: ServerId, user_id: UserId) {
        bots.expect_find_bot()
            .returning(move |_| ok(Some(bot_of(server_id, user_id))));
    }

    /// Everything a creation needs, up to the cap of `active` bots.
    fn expect_creation(fx: &mut Fx, active: u64) {
        fx.bots.expect_lock_server_bots().returning(|_| ok(()));
        fx.bots.expect_count_active().returning(move |_| ok(active));
        fx.bots
            .expect_create_bot()
            .returning(|new| ok(bot_of(new.server_id, UserId::new())));
        fx.bots
            .expect_create_token()
            .returning(|new| ok(token_of(new.bot_user_id)));
        fx.members
            .expect_add_member()
            .withf(|_, _, role| *role == ServerMemberRole::Guest)
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
    }

    async fn create(svc: &DefaultBotService, server_id: ServerId) -> ServiceResult<CreatedBotDto> {
        svc.create_bot(
            actor(),
            server_id,
            "Helper".to_string(),
            "Ada".to_string(),
            "Nowhere".to_string(),
        )
        .await
    }

    fn assert_validation<T: std::fmt::Debug>(result: ServiceResult<T>) {
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "{result:?}"
        );
    }

    fn assert_denied<T: std::fmt::Debug>(result: ServiceResult<T>) {
        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "{result:?}"
        );
    }

    fn assert_feature_off<T: std::fmt::Debug>(result: ServiceResult<T>) {
        assert!(
            matches!(
                result,
                Err(ServiceError::FeatureDisabled(ServerFeature::Bots))
            ),
            "{result:?}"
        );
    }

    // ── Gates ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn every_call_refuses_while_the_feature_is_off_before_touching_a_repository() {
        let svc = Fx::new().feature_off().build();
        let id = UserId::new();
        assert_feature_off(create(&svc, ServerId::new()).await);
        assert_feature_off(svc.update_bot(actor(), id, None, None, None).await);
        assert_feature_off(svc.deactivate_bot(actor(), id).await);
        assert_feature_off(svc.create_token(actor(), id, "ci".to_string()).await);
        assert_feature_off(svc.revoke_token(actor(), BotTokenId::new()).await);
        assert_feature_off(svc.list_bots(actor(), ServerId::new()).await);
        assert_feature_off(svc.list_tokens(actor(), id).await);
    }

    #[tokio::test]
    async fn a_member_guest_or_outsider_cannot_create_a_bot() {
        for role in [
            Some(ServerMemberRole::Member),
            Some(ServerMemberRole::Guest),
            None,
        ] {
            let svc = Fx::new().with_role(role).build();
            assert_denied(create(&svc, ServerId::new()).await);
        }
    }

    #[tokio::test]
    async fn the_direct_space_has_no_bots_whoever_asks() {
        let direct = ServerId::new();
        let mut fx = Fx::new();
        fx.direct_space = Some(direct);
        let svc = fx.build();
        assert_validation(create(&svc, direct).await);
    }

    #[tokio::test]
    async fn bad_fields_are_refused_after_the_standing_and_before_any_write() {
        let long = "x".repeat(MAX_DISCLOSURE_CHARS + 1);
        for (name, run_by, reads_go_to) in [
            ("", "Ada", "Nowhere"),
            ("\u{200B} ", "Ada", "Nowhere"),
            ("Rim-Admin", "Ada", "Nowhere"),
            ("Helper", "", "Nowhere"),
            ("Helper", "   ", "Nowhere"),
            ("Helper", "Ada", ""),
            ("Helper", long.as_str(), "Nowhere"),
            ("Helper", "Ada", long.as_str()),
            ("Helper", "A\nda", "Nowhere"),
            ("Helper", "Ada", "No\u{0007}where"),
            ("Helper", "Ada\u{202E}", "Nowhere"),
        ] {
            let svc = Fx::new()
                .with_role(Some(ServerMemberRole::Moderator))
                .build();
            let result = svc
                .create_bot(
                    actor(),
                    ServerId::new(),
                    name.to_string(),
                    run_by.to_string(),
                    reads_go_to.to_string(),
                )
                .await;
            assert_validation(result);
        }
    }

    // ── create_bot ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_moderator_creates_a_guest_in_no_channel_with_a_shown_token() {
        let server_id = ServerId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        expect_creation(&mut fx, MAX_ACTIVE_BOTS - 1);
        let audit = fx.audit_handle();
        let events = fx.events_handle();
        let svc = fx.build();

        let created = create(&svc, server_id).await.expect("created");

        assert!(is_bot_token_shape(&created.token), "{}", created.token);
        assert!(!format!("{created:?}").contains(&created.token));
        let events = events.0.lock().expect("lock");
        assert!(matches!(
            events.as_slice(),
            [WorkspaceEvent::MemberJoined { workspace_id, source: None, .. }]
                if *workspace_id == server_id
        ));
        let entries = audit.0.lock().expect("lock");
        let [entry] = entries.as_slice() else {
            panic!("{entries:?}");
        };
        assert_eq!(entry.command_id, AUDIT_CREATE);
        assert_eq!(entry.initiator, "palette");
        let args = entry.args_redacted.to_string();
        assert!(!args.contains(&created.token));
        assert!(!args.contains(&hash_token(&created.token)));
        assert!(args.contains("bot_user_id") && args.contains("token_id"));
    }

    #[tokio::test]
    async fn the_stored_hash_is_the_tokens_sha256_and_the_row_keeps_the_disclosure() {
        let hash = Arc::new(Mutex::new(String::new()));
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        fx.bots.expect_lock_server_bots().returning(|_| ok(()));
        fx.bots.expect_count_active().returning(|_| ok(0));
        fx.bots
            .expect_create_bot()
            .withf(|new| {
                new.name == "Helper"
                    && new.run_by == "Ada"
                    && new.reads_go_to == "Nowhere"
                    && new.username.starts_with("bot-helper-")
            })
            .returning(|new| ok(bot_of(new.server_id, UserId::new())));
        let seen = Arc::clone(&hash);
        fx.bots.expect_create_token().returning(move |new| {
            assert_eq!(new.label, FIRST_TOKEN_LABEL);
            *seen.lock().expect("lock") = new.token_hash.clone();
            ok(token_of(new.bot_user_id))
        });
        fx.members
            .expect_add_member()
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        let svc = fx.build();

        let created = svc
            .create_bot(
                actor(),
                ServerId::new(),
                " Helper ".to_string(),
                "  Ada ".to_string(),
                " Nowhere".to_string(),
            )
            .await
            .expect("created");

        assert_eq!(*hash.lock().expect("lock"), hash_token(&created.token));
    }

    #[tokio::test]
    async fn the_twenty_sixth_active_bot_is_refused() {
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        fx.bots.expect_lock_server_bots().returning(|_| ok(()));
        fx.bots
            .expect_count_active()
            .returning(|_| ok(MAX_ACTIVE_BOTS));
        let svc = fx.build();
        assert_validation(create(&svc, ServerId::new()).await);
    }

    #[test]
    fn a_username_keeps_the_rules_for_any_name() {
        for name in [
            "Helper",
            "r_admin",
            "deleted-account",
            "Жук",
            "🤖",
            "a very long bot name that goes on and on",
            "--",
        ] {
            let username = bot_username(name).expect("username");
            assert!(username.starts_with("bot-"), "{username}");
            assert!(username.len() <= 32, "{username}");
            assert!(!username.contains("--"), "{username}");
        }
        assert_ne!(
            bot_username("Helper").expect("username"),
            bot_username("Helper").expect("username")
        );
    }

    // ── update_bot ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_member_cannot_change_a_bot_and_an_unknown_bot_is_not_found() {
        let id = UserId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Member));
        expect_bot(&mut fx.bots, ServerId::new(), id);
        let svc = fx.build();
        assert_denied(svc.update_bot(actor(), id, None, None, None).await);

        let mut fx = Fx::new();
        fx.bots.expect_find_bot().returning(|_| ok(None));
        let svc = fx.build();
        assert!(matches!(
            svc.update_bot(actor(), id, None, None, None).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn an_update_validates_like_a_creation_and_passes_only_what_was_given() {
        let id = UserId::new();
        let server_id = ServerId::new();
        for (name, run_by, reads_go_to) in [
            (Some("Rim-Admin"), None, None),
            (None, Some(""), None),
            (None, None, Some("x\ny")),
        ] {
            let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
            expect_bot(&mut fx.bots, server_id, id);
            let svc = fx.build();
            assert_validation(
                svc.update_bot(
                    actor(),
                    id,
                    name.map(str::to_string),
                    run_by.map(str::to_string),
                    reads_go_to.map(str::to_string),
                )
                .await,
            );
        }

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots
            .expect_update_bot()
            .withf(|_, patch| {
                patch.name.is_none()
                    && patch.run_by.as_deref() == Some("Grace")
                    && patch.reads_go_to.is_none()
            })
            .returning(move |user_id, _| ok(bot_of(server_id, user_id)));
        let audit = fx.audit_handle();
        let events = fx.events_handle();
        let svc = fx.build();
        svc.update_bot(actor(), id, None, Some(" Grace ".to_string()), None)
            .await
            .expect("updated");
        assert!(events.0.lock().expect("lock").is_empty());
        assert_eq!(audit.0.lock().expect("lock")[0].command_id, AUDIT_UPDATE);
    }

    #[tokio::test]
    async fn a_rename_tells_the_clients() {
        let id = UserId::new();
        let server_id = ServerId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots
            .expect_update_bot()
            .returning(move |user_id, _| ok(bot_of(server_id, user_id)));
        let events = fx.events_handle();
        let svc = fx.build();
        svc.update_bot(actor(), id, Some("New".to_string()), None, None)
            .await
            .expect("updated");
        assert!(matches!(
            events.0.lock().expect("lock").as_slice(),
            [WorkspaceEvent::MemberProfileUpdated { .. }]
        ));
    }

    // ── deactivate_bot ───────────────────────────────────────────────────

    #[tokio::test]
    async fn deactivating_revokes_every_token_and_removes_it_through_the_kick() {
        let id = UserId::new();
        let server_id = ServerId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots
            .expect_set_deactivated()
            .times(1)
            .returning(|_, _| ok(true));
        fx.bots
            .expect_revoke_all_tokens()
            .times(1)
            .returning(|_, _| ok(2));
        fx.members.expect_is_member().returning(|_, _| Ok(true));
        fx.moderation
            .expect_kick()
            .withf(move |cmd| cmd.server_id == server_id && cmd.user_id == id)
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        let audit = fx.audit_handle();
        let svc = fx.build();

        svc.deactivate_bot(actor(), id).await.expect("deactivated");

        let entries = audit.0.lock().expect("lock");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command_id, AUDIT_DEACTIVATE);
        assert_eq!(
            entries[0].args_redacted,
            serde_json::json!({ "bot_user_id": id.to_string() })
        );
    }

    #[tokio::test]
    async fn deactivating_again_changes_nothing_and_writes_no_audit_entry() {
        let id = UserId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        expect_bot(&mut fx.bots, ServerId::new(), id);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots.expect_set_deactivated().returning(|_, _| ok(false));
        fx.bots.expect_revoke_all_tokens().returning(|_, _| ok(0));
        fx.members.expect_is_member().returning(|_, _| Ok(false));
        let audit = fx.audit_handle();
        let svc = fx.build();

        svc.deactivate_bot(actor(), id).await.expect("idempotent");

        assert!(audit.0.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_failed_removal_is_reported_after_the_tokens_are_dead() {
        let id = UserId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        expect_bot(&mut fx.bots, ServerId::new(), id);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots.expect_set_deactivated().returning(|_, _| ok(true));
        fx.bots
            .expect_revoke_all_tokens()
            .times(1)
            .returning(|_, _| ok(1));
        fx.members.expect_is_member().returning(|_, _| Ok(true));
        fx.moderation.expect_kick().returning(|_| {
            Box::pin(async { Err(ServiceError::internal(anyhow::anyhow!("down"))) })
        });
        let svc = fx.build();
        assert!(matches!(
            svc.deactivate_bot(actor(), id).await,
            Err(ServiceError::Internal(_))
        ));
    }

    // ── Tokens ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_token_is_created_up_to_the_cap_with_a_checked_label() {
        let id = UserId::new();
        let server_id = ServerId::new();

        for label in ["", "   ", &"x".repeat(MAX_TOKEN_LABEL_CHARS + 1), "a\tb"] {
            let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
            expect_bot(&mut fx.bots, server_id, id);
            let svc = fx.build();
            assert_validation(svc.create_token(actor(), id, label.to_string()).await);
        }

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots
            .expect_count_active_tokens()
            .returning(|_| ok(MAX_ACTIVE_TOKENS));
        let svc = fx.build();
        assert_validation(svc.create_token(actor(), id, "ci".to_string()).await);

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots
            .expect_count_active_tokens()
            .returning(|_| ok(MAX_ACTIVE_TOKENS - 1));
        fx.bots
            .expect_create_token()
            .withf(|new| new.label == "ci")
            .returning(|new| ok(token_of(new.bot_user_id)));
        let audit = fx.audit_handle();
        let svc = fx.build();
        let created = svc
            .create_token(actor(), id, " ci ".to_string())
            .await
            .expect("created");
        assert!(is_bot_token_shape(&created.token));
        assert!(!format!("{created:?}").contains(&created.token));
        let entries = audit.0.lock().expect("lock");
        assert_eq!(entries[0].command_id, AUDIT_TOKEN_CREATE);
        assert!(!entries[0].args_redacted.to_string().contains("rimbot_"));
    }

    #[tokio::test]
    async fn a_deactivated_bot_gets_no_new_token() {
        let id = UserId::new();
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        let server_id = ServerId::new();
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots.expect_find_bot().returning(move |user_id| {
            ok(Some(Bot {
                deactivated_at: Some(Utc::now()),
                ..bot_of(server_id, user_id)
            }))
        });
        let svc = fx.build();
        assert_validation(svc.create_token(actor(), id, "ci".to_string()).await);
    }

    #[tokio::test]
    async fn revoking_answers_not_found_outside_the_actors_workspace() {
        let id = UserId::new();
        let token = token_of(id);
        for role in [None, Some(ServerMemberRole::Member)] {
            let mut fx = Fx::new().with_role(role);
            let found = token.clone();
            fx.bots
                .expect_find_token()
                .returning(move |_| ok(Some(found.clone())));
            expect_bot(&mut fx.bots, ServerId::new(), id);
            let svc = fx.build();
            assert!(matches!(
                svc.revoke_token(actor(), token.id).await,
                Err(ServiceError::NotFound)
            ));
        }

        let mut fx = Fx::new();
        fx.bots.expect_find_token().returning(|_| ok(None));
        let svc = fx.build();
        assert!(matches!(
            svc.revoke_token(actor(), token.id).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn revoking_is_idempotent_and_audited_once() {
        let id = UserId::new();
        let token = token_of(id);
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let found = token.clone();
        fx.bots
            .expect_find_token()
            .returning(move |_| ok(Some(found.clone())));
        expect_bot(&mut fx.bots, ServerId::new(), id);
        let revoked = BotToken {
            revoked_at: Some(Utc::now()),
            ..token.clone()
        };
        fx.bots
            .expect_revoke_token()
            .times(1)
            .returning(move |_, _| ok(Some(revoked.clone())));
        let audit = fx.audit_handle();
        let svc = fx.build();
        let dto = svc.revoke_token(actor(), token.id).await.expect("revoked");
        assert!(dto.revoked_at.is_some());
        {
            let entries = audit.0.lock().expect("lock");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].command_id, AUDIT_TOKEN_REVOKE);
            assert_eq!(entries[0].args_redacted["label"], "default");
        }

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let already = BotToken {
            revoked_at: Some(Utc::now()),
            ..token.clone()
        };
        fx.bots
            .expect_find_token()
            .returning(move |_| ok(Some(already.clone())));
        expect_bot(&mut fx.bots, ServerId::new(), id);
        let audit = fx.audit_handle();
        let svc = fx.build();
        svc.revoke_token(actor(), token.id).await.expect("again");
        assert!(audit.0.lock().expect("lock").is_empty());
    }

    // ── Lists ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn any_member_lists_bots_and_only_moderators_list_tokens() {
        let id = UserId::new();
        let server_id = ServerId::new();

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Member));
        fx.bots
            .expect_list_by_server()
            .returning(move |_| ok(vec![bot_of(server_id, id)]));
        let svc = fx.build();
        assert_eq!(
            svc.list_bots(actor(), server_id).await.expect("bots").len(),
            1
        );

        for role in [None, Some(ServerMemberRole::Guest)] {
            let svc = Fx::new().with_role(role).build();
            assert_denied(svc.list_bots(actor(), server_id).await);
        }

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Member));
        expect_bot(&mut fx.bots, server_id, id);
        let svc = fx.build();
        assert_denied(svc.list_tokens(actor(), id).await);

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        expect_bot(&mut fx.bots, server_id, id);
        fx.bots
            .expect_list_tokens()
            .returning(move |_| ok(vec![token_of(id)]));
        let svc = fx.build();
        assert_eq!(svc.list_tokens(actor(), id).await.expect("tokens").len(), 1);
    }

    #[tokio::test]
    async fn bot_infos_answer_while_the_feature_is_off() {
        let id = UserId::new();
        let mut fx = Fx::new().feature_off();
        fx.bots
            .expect_list_by_users()
            .returning(move |_| ok(vec![bot_of(ServerId::new(), id)]));
        let svc = fx.build();
        let infos = svc.bot_infos(&[id]).await.expect("infos");
        assert_eq!(infos[0].user_id, id);
        assert_eq!(infos[0].run_by, "Ada");
    }

    // ── authenticate_token ───────────────────────────────────────────────

    fn good_token() -> String {
        format!("rimbot_{GOOD_TOKEN_BODY}")
    }

    fn holder(user_id: UserId, server_id: ServerId) -> BotTokenHolder {
        BotTokenHolder {
            token: token_of(user_id),
            server_id,
            bot_deactivated_at: None,
        }
    }

    /// An authentication of [`good_token`] held by `found`, for a bot in the workspace and not
    /// banned unless the test sets that up in `configure`.
    async fn authenticate(
        found: Option<BotTokenHolder>,
        configure: impl FnOnce(&mut Fx),
    ) -> Option<BotActorDto> {
        let mut fx = Fx::new();
        let expected_hash = hash_token(&good_token());
        fx.bots
            .expect_find_token_holder()
            .withf(move |hash| hash == expected_hash)
            .returning(move |_| ok(found.clone()));
        configure(&mut fx);
        fx.build()
            .authenticate_token(&good_token())
            .await
            .expect("answered")
    }

    fn in_workspace(fx: &mut Fx) {
        fx.members.expect_is_member().returning(|_, _| Ok(true));
    }

    fn not_banned(fx: &mut Fx) {
        fx.moderation_repo
            .expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
    }

    #[tokio::test]
    async fn a_wrong_shape_reaches_no_repository() {
        let svc = Fx::new().build();
        for raw in [
            "",
            "rimbot_",
            "rimbot_short",
            "rim_sess_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE",
            "rimhook_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE",
            &format!("rimbot_{GOOD_TOKEN_BODY}x"),
            &format!("rimbot_{}", GOOD_TOKEN_BODY.replacen('a', "+", 1)),
            &format!(" rimbot_{GOOD_TOKEN_BODY}"),
        ] {
            assert_eq!(svc.authenticate_token(raw).await.expect("answered"), None);
        }
    }

    #[tokio::test]
    async fn the_feature_off_refuses_a_good_token_without_a_lookup() {
        let svc = Fx::new().feature_off().build();
        assert_eq!(
            svc.authenticate_token(&good_token())
                .await
                .expect("answered"),
            None
        );
    }

    #[tokio::test]
    async fn an_unknown_token_is_refused() {
        assert_eq!(authenticate(None, |_| {}).await, None);
    }

    #[tokio::test]
    async fn a_revoked_token_is_refused() {
        let mut found = holder(UserId::new(), ServerId::new());
        found.token.revoked_at = Some(Utc::now());
        assert_eq!(authenticate(Some(found), |_| {}).await, None);
    }

    #[tokio::test]
    async fn a_token_of_a_deactivated_bot_is_refused() {
        let mut found = holder(UserId::new(), ServerId::new());
        found.bot_deactivated_at = Some(Utc::now());
        assert_eq!(authenticate(Some(found), |_| {}).await, None);
    }

    #[tokio::test]
    async fn a_token_of_a_bot_removed_from_its_workspace_is_refused() {
        let found = holder(UserId::new(), ServerId::new());
        let answer = authenticate(Some(found), |fx| {
            fx.members.expect_is_member().returning(|_, _| Ok(false));
        })
        .await;
        assert_eq!(answer, None);
    }

    #[tokio::test]
    async fn a_token_of_a_banned_bot_is_refused() {
        let found = holder(UserId::new(), ServerId::new());
        let answer = authenticate(Some(found), |fx| {
            in_workspace(fx);
            fx.moderation_repo
                .expect_get_active_ban()
                .returning(|server_id, user_id, _| {
                    Box::pin(async move {
                        Ok(Some(ServerBan {
                            server_id,
                            user_id,
                            reason: None,
                            banned_by: None,
                            created_at: Utc::now(),
                            expires_at: None,
                        }))
                    })
                });
        })
        .await;
        assert_eq!(answer, None);
    }

    #[tokio::test]
    async fn a_good_token_answers_who_it_acts_as_and_records_a_first_use() {
        let user_id = UserId::new();
        let server_id = ServerId::new();
        let found = holder(user_id, server_id);
        let token_id = found.token.id;
        let answer = authenticate(Some(found), |fx| {
            in_workspace(fx);
            not_banned(fx);
            fx.bots
                .expect_touch_token()
                .withf(move |id, _| *id == token_id)
                .times(1)
                .returning(|_, _| ok(()));
        })
        .await;
        assert_eq!(
            answer,
            Some(BotActorDto {
                user_id,
                server_id,
                token_id,
            })
        );
    }

    #[tokio::test]
    async fn the_last_use_is_recorded_at_most_once_a_minute() {
        let mut found = holder(UserId::new(), ServerId::new());
        found.token.last_used_at = Some(Utc::now() - Duration::seconds(30));
        let answer = authenticate(Some(found), |fx| {
            in_workspace(fx);
            not_banned(fx);
            fx.bots.expect_touch_token().never();
        })
        .await;
        assert!(answer.is_some());

        let mut found = holder(UserId::new(), ServerId::new());
        found.token.last_used_at = Some(Utc::now() - Duration::seconds(61));
        let answer = authenticate(Some(found), |fx| {
            in_workspace(fx);
            not_banned(fx);
            fx.bots
                .expect_touch_token()
                .times(1)
                .returning(|_, _| ok(()));
        })
        .await;
        assert!(answer.is_some());
    }

    // ── What a working token reaches ─────────────────────────────────────

    fn actor_of(server_id: ServerId) -> BotActorDto {
        BotActorDto {
            user_id: UserId::new(),
            server_id,
            token_id: BotTokenId::new(),
        }
    }

    fn channel_of(server_id: ServerId) -> rim_core::conversation::model::Conversation {
        rim_core::conversation::model::Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: ConversationType::Channel,
            title: Some("ops".to_string()),
            topic: None,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            posting_policy: rim_core::conversation::model::PostingPolicy::Everyone,
            posting_allows_threads: true,
            encrypted_at: None,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
        }
    }

    /// A service whose conversation repositories answer `conversation` and `member`.
    fn with_conversation(
        conversation: Option<rim_core::conversation::model::Conversation>,
        member: bool,
    ) -> DefaultBotService {
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_get_by_id()
            .returning(move |_| ok(conversation.clone()));
        let mut members = MockConversationMemberRepository::new();
        members.expect_is_member().returning(move |_, _| ok(member));
        let fx = Fx::new();
        DefaultBotService::new(
            BotRepos {
                bot_repo: Arc::new(fx.bots),
                server_member_repo: Arc::new(fx.members),
                conversation_repo: Arc::new(conversations),
                conversation_member_repo: Arc::new(members),
                moderation_repo: Arc::new(fx.moderation_repo),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
            },
            Observers::new(fx.events),
            Arc::new(InstanceFeatureGate::new(&[], &[])),
            Arc::new(fx.moderation),
            Arc::new(MockConversationService::new()),
        )
    }

    #[tokio::test]
    async fn a_bot_is_in_a_channel_of_its_workspace_it_is_a_member_of_and_nothing_else() {
        let server_id = ServerId::new();
        let bot = actor_of(server_id);
        let id = ConversationId::new();
        let is_in = async |conversation, member| {
            with_conversation(conversation, member)
                .is_in_channel(bot, id)
                .await
                .expect("answered")
        };

        assert!(is_in(Some(channel_of(server_id)), true).await);
        assert!(!is_in(Some(channel_of(server_id)), false).await);
        assert!(!is_in(None, true).await);
        assert!(!is_in(Some(channel_of(ServerId::new())), true).await);
        let group = rim_core::conversation::model::Conversation {
            conversation_type: ConversationType::Group,
            ..channel_of(server_id)
        };
        assert!(!is_in(Some(group), true).await);
        let encrypted = rim_core::conversation::model::Conversation {
            encrypted_at: Some(Utc::now()),
            ..channel_of(server_id)
        };
        assert!(!is_in(Some(encrypted), true).await);
    }

    /// A service whose mention repository answers `answer`, with its audit log recorded.
    fn with_mentions(
        answer: RepoResult<
            Vec<(
                rim_core::message_mention::model::MessageMention,
                rim_core::message::model::Message,
            )>,
        >,
        check: impl Fn(
            Option<(DateTime<Utc>, rim_core::message::model::MessageId)>,
            DateTime<Utc>,
            u64,
        ) -> bool
        + Send
        + 'static,
    ) -> (DefaultBotService, Arc<RecordingAudit>) {
        let mut repo = MockMessageMentionRepository::new();
        let mut answer = Some(answer);
        repo.expect_list_since()
            .returning(move |_, after, since, limit| {
                assert!(check(after, since, limit));
                let answer = answer.take().expect("one call");
                Box::pin(async move { answer })
            });
        let fx = Fx::new();
        let audit = fx.audit_handle();
        let svc = DefaultBotService::new(
            BotRepos {
                bot_repo: Arc::new(fx.bots),
                server_member_repo: Arc::new(fx.members),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                moderation_repo: Arc::new(fx.moderation_repo),
                message_mention_repo: Arc::new(repo),
            },
            Observers::new(fx.events),
            Arc::new(InstanceFeatureGate::new(&[], &[])),
            Arc::new(fx.moderation),
            Arc::new(MockConversationService::new()),
        )
        .with_audit_log_service(fx.audit);
        (svc, audit)
    }

    #[tokio::test]
    async fn an_empty_poll_starts_a_day_back_without_a_cursor_and_is_not_audited() {
        let (svc, audit) = with_mentions(Ok(vec![]), |after, since, limit| {
            let age = Utc::now() - since;
            after.is_none()
                && limit == 10
                && age >= Duration::hours(FIRST_MENTIONS_WINDOW_HOURS)
                && age < Duration::hours(FIRST_MENTIONS_WINDOW_HOURS) + Duration::minutes(1)
        });
        let bot = actor_of(ServerId::new());

        let page = svc.list_mentions(bot, None, 10).await.expect("a page");

        assert!(page.messages.is_empty());
        assert_eq!(page.last, None);
        assert!(audit.0.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_poll_that_hands_over_mentions_is_audited_as_the_bots_read() {
        let bot = actor_of(ServerId::new());
        let message = rim_core::message::model::Message {
            id: rim_core::message::model::MessageId::new(),
            conversation_id: rim_core::conversation::model::ConversationId::new(),
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_at: Utc::now(),
            edited_at: None,
            sent_by: UserId::new(),
            text: Some("look".to_string()),
            is_deleted: false,
            has_attachments: false,
            encryption: None,
            flagged_at: None,
        };
        let mention = rim_core::message_mention::model::MessageMention {
            message_id: message.id,
            mentioned_user_id: bot.user_id,
            created_at: Utc::now(),
        };
        let (svc, audit) = with_mentions(Ok(vec![(mention, message)]), |_, _, _| true);

        let page = svc.list_mentions(bot, None, 10).await.expect("a page");

        assert_eq!(page.messages.len(), 1);
        let entries = audit.0.lock().expect("lock");
        let [entry] = entries.as_slice() else {
            panic!("{entries:?}");
        };
        assert_eq!(entry.command_id, AUDIT_MENTIONS);
        assert_eq!(entry.initiator, "bot");
        assert_eq!(entry.sensitivity, "safe");
        assert_eq!(entry.outcome, AuditOutcome::Success);
        assert_eq!(entry.typed_verb, Some(TypedVerb::Read));
    }

    #[tokio::test]
    async fn a_failed_read_of_mentions_is_audited_and_returned() {
        let (svc, audit) = with_mentions(
            Err(RepoError::new(
                rim_core::repository::error::RepoErrorKind::Internal,
            )),
            |_, _, _| true,
        );
        let answer = svc.list_mentions(actor_of(ServerId::new()), None, 10).await;
        assert!(answer.is_err());
        let entries = audit.0.lock().expect("lock");
        assert_eq!(entries[0].outcome, AuditOutcome::Error);
        assert!(entries[0].error_kind.is_some());
    }

    #[tokio::test]
    async fn a_failed_last_use_write_does_not_refuse_the_token() {
        let found = holder(UserId::new(), ServerId::new());
        let answer = authenticate(Some(found), |fx| {
            in_workspace(fx);
            not_banned(fx);
            fx.bots.expect_touch_token().returning(|_, _| {
                Box::pin(async {
                    Err(RepoError::new(
                        rim_core::repository::error::RepoErrorKind::Internal,
                    ))
                })
            });
        })
        .await;
        assert!(answer.is_some());
    }
}
