//! Roles and moderation (Track 1.2, `docs/specs/track-1-2-roles-and-moderation.md`).
//!
//! [`DefaultModerationService`] implements role changes, kick, ban, timeout, slow mode and
//! message reports. [`record_moderation_action`] is the one place a moderation action is
//! written to `command_audit_log`; the message service uses it too, for moderator deletes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::info;

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::moderation::model::{
    MemberTimeout, MessageReport, MessageReportStatus, NewMessageReport, ServerBan,
};
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberRole};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::verb::TypedVerb;
use rim_services::conversation::dto::ConversationDto;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::moderation::command::{
    BanMemberCmd, ClearTimeoutCmd, KickMemberCmd, ListBansQuery, ListReportsQuery,
    ReportMessageCmd, ResolveReportCmd, SetMemberRoleCmd, SetSlowModeCmd, TimeoutMemberCmd,
    UnbanMemberCmd,
};
use rim_services::moderation::dto::{
    MemberTimeoutDto, MessageReportDto, ReportPageCursor, ServerBanDto,
};
use rim_services::moderation::service::ModerationService;
use rim_services::moderation::{MAX_SLOW_MODE_SECONDS, MAX_TIMEOUT_DAYS};
use rim_services::server_member::dto::ServerMemberDto;
use rim_services::{ServiceError, ServiceResult};

use crate::util::{
    ConversationAccessRepos, ensure_conversation_in_server, ensure_conversation_member_access,
    ensure_not_archived,
};

/// Pseudo command ids under which moderation actions are recorded in `command_audit_log`, in
/// the style of `workspace.set_feature`. Some coincide with the ids of the builtin commands that
/// call the same service method; a command execution then has two rows, the generic command
/// row and this one (initiator `moderation`).
pub(crate) mod audit_ids {
    pub(crate) const KICK: &str = "admin.kick";
    pub(crate) const BAN: &str = "admin.ban";
    pub(crate) const UNBAN: &str = "admin.unban";
    pub(crate) const TIMEOUT: &str = "admin.timeout";
    pub(crate) const CLEAR_TIMEOUT: &str = "admin.clear_timeout";
    pub(crate) const SET_SLOW_MODE: &str = "chat.set_slow_mode";
    pub(crate) const REPORT_MESSAGE: &str = "chat.report_message";
    pub(crate) const RESOLVE_REPORT: &str = "admin.resolve_report";
    pub(crate) const SET_MEMBER_ROLE: &str = "admin.set_member_role";
    pub(crate) const DELETE_MESSAGE: &str = "admin.delete_message";
}

/// `initiator` column value of moderation audit rows.
pub(crate) const MODERATION_AUDIT_INITIATOR: &str = "moderation";

/// Longest accepted report reason, in characters.
const MAX_REPORT_REASON_CHARS: usize = 1000;
/// Longest accepted ban/timeout reason, in characters.
const MAX_MODERATION_REASON_CHARS: usize = 512;
/// Maximum page size for `list_reports`.
const MAX_REPORT_PAGE_LIMIT: u32 = 200;

/// One moderation action to audit.
pub(crate) struct ModerationAuditEntry {
    pub(crate) actor: UserId,
    pub(crate) server_id: ServerId,
    /// One of [`audit_ids`].
    pub(crate) command_id: &'static str,
    /// Ids and flags only; never free text (reasons, message bodies).
    pub(crate) args: serde_json::Value,
    pub(crate) dangerous: bool,
    pub(crate) typed_verb: TypedVerb,
}

/// The shared audit writer for every moderation action, whichever surface it came from
/// (GraphQL or a command): a `target: "audit"` tracing line and, when wired, a
/// `command_audit_log` row. A failed DB write is logged and swallowed; the action itself
/// already succeeded.
pub(crate) async fn record_moderation_action(
    audit_log_service: Option<&Arc<dyn AuditLogService>>,
    entry: ModerationAuditEntry,
) {
    tracing::info!(
        target: "audit",
        event = "moderation_action",
        actor_id = %entry.actor,
        server_id = %entry.server_id,
        command_id = entry.command_id,
        args = %entry.args,
    );
    let Some(audit_svc) = audit_log_service else {
        return;
    };
    let row = AuditLogEntryDto {
        id: uuid::Uuid::now_v7(),
        actor_user_id: entry.actor.into(),
        server_id: entry.server_id.into(),
        command_id: entry.command_id.to_string(),
        initiator: MODERATION_AUDIT_INITIATOR.to_string(),
        args_redacted: entry.args,
        sensitivity: "restricted".to_string(),
        dangerous: entry.dangerous,
        confirmation_kind: "none".to_string(),
        typed_verb: Some(entry.typed_verb),
        outcome: AuditOutcome::Success,
        error_kind: None,
        capability_check_outcome: None,
        confirmation_outcome: None,
        parent_audit_id: None,
        created_at: Utc::now(),
    };
    if let Err(e) = audit_svc.write_entry(row).await {
        tracing::warn!(
            event = "audit_db_write_failed",
            error = %e,
            command_id = entry.command_id,
            "audit DB write failed for moderation action (tracing line already emitted)"
        );
    }
}

/// `Ok(())` when a member with `actor_role` may kick, ban or time out a member with
/// `target_role`: the actor must be Owner or Moderator, nobody targets the Owner, and a
/// Moderator cannot target another Moderator.
pub(crate) fn ensure_can_moderate(
    actor_role: ServerMemberRole,
    target_role: ServerMemberRole,
) -> ServiceResult<()> {
    if !actor_role.is_moderator() {
        return Err(ServiceError::PermissionDenied);
    }
    match (actor_role, target_role) {
        (_, ServerMemberRole::Owner) => Err(ServiceError::PermissionDenied),
        (ServerMemberRole::Moderator, ServerMemberRole::Moderator) => {
            Err(ServiceError::PermissionDenied)
        }
        _ => Ok(()),
    }
}

/// `Ok(())` when `actor_role` may move a member from `current` to `new`. Owner: any role but
/// `Owner`, on anyone but another Owner. Moderator: `Member` <-> `Guest` only.
pub(crate) fn ensure_can_set_role(
    actor_role: ServerMemberRole,
    current: ServerMemberRole,
    new: ServerMemberRole,
) -> ServiceResult<()> {
    if new == ServerMemberRole::Owner {
        return Err(ServiceError::Validation(
            "the owner role cannot be granted".to_string(),
        ));
    }
    let member_or_guest =
        |r: ServerMemberRole| matches!(r, ServerMemberRole::Member | ServerMemberRole::Guest);
    match actor_role {
        ServerMemberRole::Owner if current != ServerMemberRole::Owner => Ok(()),
        ServerMemberRole::Moderator if member_or_guest(current) && member_or_guest(new) => Ok(()),
        _ => Err(ServiceError::PermissionDenied),
    }
}

fn normalize_reason(reason: Option<String>, max_chars: usize) -> ServiceResult<Option<String>> {
    let Some(reason) = reason else {
        return Ok(None);
    };
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_chars {
        return Err(ServiceError::Validation(format!(
            "reason must be at most {max_chars} characters"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

#[derive(Clone)]
pub struct ModerationRepos {
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
}

pub struct DefaultModerationService {
    repos: ModerationRepos,
    observers: Observers,
    /// `None` in unit tests; the composition root always wires it.
    audit_log_service: Option<Arc<dyn AuditLogService>>,
}

impl DefaultModerationService {
    pub fn new(repos: ModerationRepos, observers: Observers) -> Self {
        Self {
            repos,
            observers,
            audit_log_service: None,
        }
    }

    pub fn with_audit_log_service(mut self, audit_log_service: Arc<dyn AuditLogService>) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    async fn audit(&self, entry: ModerationAuditEntry) {
        record_moderation_action(self.audit_log_service.as_ref(), entry).await;
    }

    fn access_repos(&self) -> ConversationAccessRepos<'_> {
        ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        )
    }

    /// The actor's membership; `PermissionDenied` for a non-member.
    async fn actor_member(
        &self,
        server_id: ServerId,
        actor: UserId,
    ) -> ServiceResult<ServerMember> {
        self.repos
            .server_member_repo
            .get_member(server_id, actor)
            .await?
            .ok_or(ServiceError::PermissionDenied)
    }

    /// The actor's membership, which must be Owner or Moderator.
    async fn actor_moderator(
        &self,
        server_id: ServerId,
        actor: UserId,
    ) -> ServiceResult<ServerMember> {
        let member = self.actor_member(server_id, actor).await?;
        if !member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(member)
    }

    /// Shared prologue of kick/ban/timeout: the actor is a moderator, does not target
    /// themselves, and may act on the target (when the target is a member). Returns the
    /// target's membership, if any.
    async fn authorize_target(
        &self,
        server_id: ServerId,
        actor: UserId,
        target: UserId,
    ) -> ServiceResult<Option<ServerMember>> {
        let actor_member = self.actor_moderator(server_id, actor).await?;
        if actor == target {
            return Err(ServiceError::Validation(
                "you cannot moderate yourself".to_string(),
            ));
        }
        let target_member = self
            .repos
            .server_member_repo
            .get_member(server_id, target)
            .await?;
        if let Some(target_member) = &target_member {
            ensure_can_moderate(actor_member.member_role, target_member.member_role)?;
        }
        Ok(target_member)
    }

    /// Removes the user from the workspace and from every conversation of it (so a later
    /// re-join does not restore private-channel access), then announces `MemberLeft`.
    async fn remove_from_workspace(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> ServiceResult<()> {
        self.repos
            .server_member_repo
            .remove_member(server_id, user_id)
            .await?;
        let joined = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(server_id, user_id)
            .await?;
        for conversation_id in joined {
            match self
                .repos
                .conversation_member_repo
                .remove_member(conversation_id, user_id)
                .await
            {
                Ok(()) => {}
                Err(e) if matches!(e.kind(), RepoErrorKind::NotFound) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.observers.emit(WorkspaceEvent::MemberLeft {
            workspace_id: server_id,
            user_id,
        });
        Ok(())
    }

    async fn to_conversation_dto(
        &self,
        conversation: Conversation,
    ) -> ServiceResult<ConversationDto> {
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await?;
        Ok(ConversationDto {
            id: conversation.id,
            server_id: conversation.server_id,
            conversation_type: conversation.conversation_type,
            title: conversation.title,
            topic: conversation.topic,
            member_ids,
            is_private: conversation.is_private,
            archived_at: conversation.archived_at,
            created_by: conversation.created_by,
            slow_mode_seconds: conversation.slow_mode_seconds,
        })
    }

    async fn member_dto(&self, member: ServerMember) -> ServiceResult<ServerMemberDto> {
        let user = self
            .repos
            .user_repo
            .get_by_id(member.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let timeout_until = self
            .repos
            .moderation_repo
            .get_active_timeout(member.server_id, member.user_id, Utc::now())
            .await?
            .map(|t| t.until);
        Ok(ServerMemberDto {
            server_id: member.server_id,
            user_id: member.user_id,
            username: user.username,
            display_name: user.display_name,
            avatar_url: user.avatar_url,
            status_text: user.status_text,
            bio: user.bio,
            member_role: member.member_role,
            joined_at: member.joined_at,
            timeout_until,
            principal_id: user.principal_id,
        })
    }

    /// Reports with the message details the moderator may see: the body and author only when
    /// the moderator can read the conversation (public channel, or conversation member).
    async fn reports_with_messages(
        &self,
        moderator: UserId,
        reports: Vec<MessageReport>,
    ) -> ServiceResult<Vec<MessageReportDto>> {
        let conversation_ids: Vec<ConversationId> = reports
            .iter()
            .map(|r| r.conversation_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let conversations: HashMap<ConversationId, Conversation> = self
            .repos
            .conversation_repo
            .get_by_ids(&conversation_ids)
            .await?
            .into_iter()
            .map(|c| (c.id, c))
            .collect();
        let mut readable: HashSet<ConversationId> = HashSet::new();
        for (id, conversation) in &conversations {
            if conversation.is_public_channel()
                || self
                    .repos
                    .conversation_member_repo
                    .is_member(*id, moderator)
                    .await?
            {
                readable.insert(*id);
            }
        }
        let readable_message_ids: Vec<_> = reports
            .iter()
            .filter(|r| readable.contains(&r.conversation_id))
            .map(|r| r.message_id)
            .collect();
        let messages = if readable_message_ids.is_empty() {
            HashMap::new()
        } else {
            self.repos
                .message_repo
                .get_by_ids(&readable_message_ids)
                .await?
        };

        Ok(reports
            .into_iter()
            .map(|report| {
                let is_readable = readable.contains(&report.conversation_id);
                let message = messages.get(&report.message_id).cloned();
                let mut dto = MessageReportDto::without_message(report);
                if is_readable {
                    dto.message_readable = true;
                    if let Some(message) = message {
                        dto.message_author_id = Some(message.sent_by);
                        dto.message_text = if message.is_deleted {
                            None
                        } else {
                            message.text
                        };
                    }
                }
                dto
            })
            .collect())
    }
}

#[async_trait]
impl ModerationService for DefaultModerationService {
    async fn set_member_role(&self, cmd: SetMemberRoleCmd) -> ServiceResult<ServerMemberDto> {
        let actor_member = self.actor_member(cmd.server_id, cmd.actor.user_id).await?;
        if !actor_member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        if cmd.user_id == cmd.actor.user_id {
            return Err(ServiceError::Validation(
                "you cannot change your own role".to_string(),
            ));
        }
        let target = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_can_set_role(actor_member.member_role, target.member_role, cmd.role)?;

        if target.member_role == cmd.role {
            return self.member_dto(target).await;
        }
        let previous = target.member_role;
        self.repos
            .server_member_repo
            .change_member_role(cmd.server_id, cmd.user_id, cmd.role)
            .await?;
        info!(
            server_id = %cmd.server_id,
            user_id = %cmd.user_id,
            role = cmd.role.as_str(),
            "changed member role"
        );

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::SET_MEMBER_ROLE,
            args: serde_json::json!({
                "user_id": cmd.user_id.to_string(),
                "previous_role": previous.as_str(),
                "role": cmd.role.as_str(),
            }),
            dangerous: true,
            typed_verb: TypedVerb::Configure,
        })
        .await;
        self.observers.emit(WorkspaceEvent::MemberRoleChanged {
            workspace_id: cmd.server_id,
            user_id: cmd.user_id,
            role: cmd.role.as_str().to_string(),
        });

        self.member_dto(ServerMember {
            member_role: cmd.role,
            ..target
        })
        .await
    }

    async fn kick(&self, cmd: KickMemberCmd) -> ServiceResult<()> {
        let reason = normalize_reason(cmd.reason, MAX_MODERATION_REASON_CHARS)?;
        let target = self
            .authorize_target(cmd.server_id, cmd.actor.user_id, cmd.user_id)
            .await?;
        if target.is_none() {
            return Err(ServiceError::NotFound);
        }
        self.remove_from_workspace(cmd.server_id, cmd.user_id)
            .await?;
        info!(server_id = %cmd.server_id, user_id = %cmd.user_id, "kicked member");

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::KICK,
            args: serde_json::json!({
                "user_id": cmd.user_id.to_string(),
                "has_reason": reason.is_some(),
            }),
            dangerous: true,
            typed_verb: TypedVerb::Delete,
        })
        .await;
        Ok(())
    }

    async fn ban(&self, cmd: BanMemberCmd) -> ServiceResult<ServerBanDto> {
        let reason = normalize_reason(cmd.reason, MAX_MODERATION_REASON_CHARS)?;
        let now = Utc::now();
        if cmd.expires_at.is_some_and(|at| at <= now) {
            return Err(ServiceError::Validation(
                "ban expiry must be in the future".to_string(),
            ));
        }
        let target = self
            .authorize_target(cmd.server_id, cmd.actor.user_id, cmd.user_id)
            .await?;
        if target.is_none() && self.repos.user_repo.get_by_id(cmd.user_id).await?.is_none() {
            return Err(ServiceError::NotFound);
        }

        // Record the ban before removing the member, so a concurrent re-join attempt already
        // sees it.
        let ban = self
            .repos
            .moderation_repo
            .upsert_ban(ServerBan {
                server_id: cmd.server_id,
                user_id: cmd.user_id,
                reason,
                banned_by: Some(cmd.actor.user_id),
                created_at: now,
                expires_at: cmd.expires_at,
            })
            .await?;
        if target.is_some() {
            self.remove_from_workspace(cmd.server_id, cmd.user_id)
                .await?;
        }
        info!(server_id = %cmd.server_id, user_id = %cmd.user_id, "banned user");

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::BAN,
            args: serde_json::json!({
                "user_id": cmd.user_id.to_string(),
                "expires_at": cmd.expires_at.map(|t| t.to_rfc3339()),
                "was_member": target.is_some(),
                "has_reason": ban.reason.is_some(),
            }),
            dangerous: true,
            typed_verb: TypedVerb::Delete,
        })
        .await;
        Ok(ban.into())
    }

    async fn unban(&self, cmd: UnbanMemberCmd) -> ServiceResult<bool> {
        self.actor_moderator(cmd.server_id, cmd.actor.user_id)
            .await?;
        let existed = self
            .repos
            .moderation_repo
            .delete_ban(cmd.server_id, cmd.user_id)
            .await?;
        if existed {
            self.audit(ModerationAuditEntry {
                actor: cmd.actor.user_id,
                server_id: cmd.server_id,
                command_id: audit_ids::UNBAN,
                args: serde_json::json!({ "user_id": cmd.user_id.to_string() }),
                dangerous: false,
                typed_verb: TypedVerb::Write,
            })
            .await;
        }
        Ok(existed)
    }

    async fn list_bans(&self, query: ListBansQuery) -> ServiceResult<Vec<ServerBanDto>> {
        self.actor_moderator(query.server_id, query.actor.user_id)
            .await?;
        Ok(self
            .repos
            .moderation_repo
            .list_active_bans(query.server_id, Utc::now())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn timeout(&self, cmd: TimeoutMemberCmd) -> ServiceResult<MemberTimeoutDto> {
        let reason = normalize_reason(cmd.reason, MAX_MODERATION_REASON_CHARS)?;
        let now = Utc::now();
        if cmd.until <= now {
            return Err(ServiceError::Validation(
                "timeout end must be in the future".to_string(),
            ));
        }
        if cmd.until > now + chrono::Duration::days(MAX_TIMEOUT_DAYS) {
            return Err(ServiceError::Validation(format!(
                "a timeout can last at most {MAX_TIMEOUT_DAYS} days"
            )));
        }
        let target = self
            .authorize_target(cmd.server_id, cmd.actor.user_id, cmd.user_id)
            .await?;
        if target.is_none() {
            return Err(ServiceError::NotFound);
        }
        let timeout = self
            .repos
            .moderation_repo
            .upsert_timeout(MemberTimeout {
                server_id: cmd.server_id,
                user_id: cmd.user_id,
                until: cmd.until,
                reason,
                set_by: Some(cmd.actor.user_id),
                created_at: now,
            })
            .await?;
        info!(server_id = %cmd.server_id, user_id = %cmd.user_id, until = %cmd.until, "timed out member");

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::TIMEOUT,
            args: serde_json::json!({
                "user_id": cmd.user_id.to_string(),
                "until": cmd.until.to_rfc3339(),
                "has_reason": timeout.reason.is_some(),
            }),
            dangerous: false,
            typed_verb: TypedVerb::Write,
        })
        .await;
        Ok(timeout.into())
    }

    async fn clear_timeout(&self, cmd: ClearTimeoutCmd) -> ServiceResult<bool> {
        self.authorize_target(cmd.server_id, cmd.actor.user_id, cmd.user_id)
            .await?;
        let active = self
            .repos
            .moderation_repo
            .get_active_timeout(cmd.server_id, cmd.user_id, Utc::now())
            .await?
            .is_some();
        // Deletes expired rows too (opportunistic cleanup).
        self.repos
            .moderation_repo
            .delete_timeout(cmd.server_id, cmd.user_id)
            .await?;
        if active {
            self.audit(ModerationAuditEntry {
                actor: cmd.actor.user_id,
                server_id: cmd.server_id,
                command_id: audit_ids::CLEAR_TIMEOUT,
                args: serde_json::json!({ "user_id": cmd.user_id.to_string() }),
                dangerous: false,
                typed_verb: TypedVerb::Write,
            })
            .await;
        }
        Ok(active)
    }

    async fn set_slow_mode(&self, cmd: SetSlowModeCmd) -> ServiceResult<ConversationDto> {
        if !(0..=MAX_SLOW_MODE_SECONDS).contains(&cmd.seconds) {
            return Err(ServiceError::Validation(format!(
                "slow mode must be between 0 and {MAX_SLOW_MODE_SECONDS} seconds"
            )));
        }
        // Role first: a non-moderator learns nothing about which ids exist (same rule as
        // archiving).
        self.actor_moderator(cmd.server_id, cmd.actor.user_id)
            .await?;
        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;
        if conversation.conversation_type != ConversationType::Channel {
            return Err(ServiceError::Validation(
                "slow mode applies to channels only".to_string(),
            ));
        }
        ensure_not_archived(&conversation)?;

        if conversation.slow_mode_seconds == cmd.seconds {
            return self.to_conversation_dto(conversation).await;
        }
        let updated = self
            .repos
            .conversation_repo
            .set_slow_mode(conversation.id, cmd.seconds)
            .await?;
        info!(conversation_id = %updated.id, seconds = cmd.seconds, "set slow mode");

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::SET_SLOW_MODE,
            args: serde_json::json!({
                "conversation_id": cmd.conversation_id.to_string(),
                "seconds": cmd.seconds,
            }),
            dangerous: false,
            typed_verb: TypedVerb::Configure,
        })
        .await;

        let dto = self.to_conversation_dto(updated).await?;
        self.observers.emit(WorkspaceEvent::ConversationUpdated {
            workspace_id: dto.server_id,
            conversation_id: dto.id,
            conversation_type: dto.conversation_type.as_str().to_string(),
            is_private: dto.is_private,
            member_user_ids: dto.member_ids.clone(),
        });
        Ok(dto)
    }

    async fn report_message(&self, cmd: ReportMessageCmd) -> ServiceResult<MessageReportDto> {
        let reason = cmd.reason.trim();
        if reason.is_empty() {
            return Err(ServiceError::Validation(
                "a report needs a reason".to_string(),
            ));
        }
        if reason.chars().count() > MAX_REPORT_REASON_CHARS {
            return Err(ServiceError::Validation(format!(
                "reason must be at most {MAX_REPORT_REASON_CHARS} characters"
            )));
        }
        // Anyone who can read the message may report it (guests only where they are members).
        ensure_conversation_member_access(
            self.access_repos(),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        let message = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if message.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }
        if message.is_deleted {
            return Err(ServiceError::Validation(
                "cannot report a deleted message".to_string(),
            ));
        }

        let report = match self
            .repos
            .moderation_repo
            .create_report(NewMessageReport {
                server_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                message_id: cmd.message_id,
                reporter_user_id: cmd.actor.user_id,
                reason: reason.to_string(),
            })
            .await
        {
            Ok(report) => report,
            Err(e) if matches!(e.kind(), RepoErrorKind::Conflict) => {
                return Err(ServiceError::Conflict(
                    "you already reported this message".to_string(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        info!(report_id = %report.id, message_id = %cmd.message_id, "message reported");

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::REPORT_MESSAGE,
            args: serde_json::json!({
                "report_id": report.id.to_string(),
                "conversation_id": cmd.conversation_id.to_string(),
                "message_id": cmd.message_id.to_string(),
            }),
            dangerous: false,
            typed_verb: TypedVerb::Write,
        })
        .await;
        Ok(MessageReportDto::without_message(report))
    }

    async fn list_reports(
        &self,
        query: ListReportsQuery,
    ) -> ServiceResult<Page<MessageReportDto, ReportPageCursor>> {
        self.actor_moderator(query.server_id, query.actor.user_id)
            .await?;
        let page = self
            .repos
            .moderation_repo
            .list_reports(
                query.server_id,
                query.status,
                query.limit.clamp(1, MAX_REPORT_PAGE_LIMIT),
                query.dir,
                query.cursor.map(Into::into),
            )
            .await?;
        let items = self
            .reports_with_messages(query.actor.user_id, page.items)
            .await?;
        Ok(Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn resolve_report(&self, cmd: ResolveReportCmd) -> ServiceResult<MessageReportDto> {
        self.actor_moderator(cmd.server_id, cmd.actor.user_id)
            .await?;
        let report = self
            .repos
            .moderation_repo
            .get_report(cmd.report_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if report.server_id != cmd.server_id {
            return Err(ServiceError::NotFound);
        }
        let status: MessageReportStatus = cmd.outcome.into();
        let resolved = match self
            .repos
            .moderation_repo
            .resolve_report(report.id, status, cmd.actor.user_id, Utc::now())
            .await
        {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), RepoErrorKind::PreconditionFailed) => {
                return Err(ServiceError::Conflict("report is not open".to_string()));
            }
            Err(e) => return Err(e.into()),
        };

        self.audit(ModerationAuditEntry {
            actor: cmd.actor.user_id,
            server_id: cmd.server_id,
            command_id: audit_ids::RESOLVE_REPORT,
            args: serde_json::json!({
                "report_id": cmd.report_id.to_string(),
                "outcome": status.as_str(),
            }),
            dangerous: false,
            typed_verb: TypedVerb::Write,
        })
        .await;

        let mut items = self
            .reports_with_messages(cmd.actor.user_id, vec![resolved])
            .await?;
        items.pop().ok_or(ServiceError::NotFound)
    }

    async fn active_timeout(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> ServiceResult<Option<DateTime<Utc>>> {
        Ok(self
            .repos
            .moderation_repo
            .get_active_timeout(server_id, user_id, Utc::now())
            .await?
            .map(|t| t.until))
    }
}
