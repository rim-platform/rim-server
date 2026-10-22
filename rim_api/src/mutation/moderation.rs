use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::moderation::command::{
    BanMemberCmd, ClearTimeoutCmd, KickMemberCmd, ReportMessageCmd, ResolveReportCmd,
    SetMemberRoleCmd, SetSlowModeCmd, TimeoutMemberCmd, UnbanMemberCmd,
};
use rim_services::moderation::service::ModerationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    Conversation, MemberTimeout, MessageReport, ReportOutcome, ServerBan, ServerMember,
    ServerMemberRoleGql,
};

#[derive(Default)]
pub(crate) struct ModerationMutations;

fn server_and_user(
    server_id: &ID,
    user_id: &ID,
) -> Result<(
    rim_core::server::model::ServerId,
    rim_core::user::model::UserId,
)> {
    Ok((parse_uuid_id(server_id)?, parse_uuid_id(user_id)?))
}

fn moderation(ctx: &Context<'_>) -> Result<std::sync::Arc<dyn ModerationService>> {
    Ok(std::sync::Arc::clone(
        ctx.data::<std::sync::Arc<dyn ModerationService>>()?,
    ))
}

#[Object]
impl ModerationMutations {
    // ── Roles and moderation ──────────────────────────────────────────

    /// Change a member's role. Owner: any role but `OWNER`, never their own. Moderator:
    /// `MEMBER` <-> `GUEST` only. Audited (`admin.set_member_role`).
    async fn set_member_role(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        user_id: ID,
        role: ServerMemberRoleGql,
    ) -> Result<ServerMember> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        let member = moderation(ctx)?
            .set_member_role(SetMemberRoleCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
                role: role.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(member.into())
    }

    /// Remove a member from the workspace (Owner/Moderator; nobody can kick the owner, and a
    /// moderator cannot kick a moderator). They may re-join with a new invite. Audited.
    async fn kick_member(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        user_id: ID,
        reason: Option<String>,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        moderation(ctx)?
            .kick(KickMemberCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
                reason,
            })
            .await
            .map_err(map_service_error)?;
        Ok(true)
    }

    /// Remove a member and block them from re-joining until `expiresAt` (or until unbanned
    /// when omitted). Same targeting rules as `kickMember`. Audited.
    async fn ban_member(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        user_id: ID,
        reason: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<ServerBan> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        let ban = moderation(ctx)?
            .ban(BanMemberCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
                reason,
                expires_at,
            })
            .await
            .map_err(map_service_error)?;
        Ok(ban.into())
    }

    /// Lift a ban (Owner/Moderator). Returns whether a ban existed. Audited.
    async fn unban_member(&self, ctx: &Context<'_>, server_id: ID, user_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        moderation(ctx)?
            .unban(UnbanMemberCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
            })
            .await
            .map_err(map_service_error)
    }

    /// Time a member out until `until` (at most 30 days ahead): they keep reading but every
    /// write fails with `TIMED_OUT`. Same targeting rules as `kickMember`. Audited.
    async fn timeout_member(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        user_id: ID,
        until: DateTime<Utc>,
        reason: Option<String>,
    ) -> Result<MemberTimeout> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        let timeout = moderation(ctx)?
            .timeout(TimeoutMemberCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
                until,
                reason,
            })
            .await
            .map_err(map_service_error)?;
        Ok(timeout.into())
    }

    /// End a member's timeout early. Returns whether one was in force. Audited.
    async fn clear_timeout(&self, ctx: &Context<'_>, server_id: ID, user_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let (server_id, user_id) = server_and_user(&server_id, &user_id)?;
        moderation(ctx)?
            .clear_timeout(ClearTimeoutCmd {
                actor: Actor::new(actor),
                server_id,
                user_id,
            })
            .await
            .map_err(map_service_error)
    }

    /// Set a channel's slow mode, in seconds (0 to 21600; 0 turns it off). Owner/Moderator.
    /// Audited (`chat.set_slow_mode`).
    async fn set_slow_mode(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        seconds: i32,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;
        let conversation = moderation(ctx)?
            .set_slow_mode(SetSlowModeCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                conversation_id: parse_uuid_id(&conversation_id)?,
                seconds,
            })
            .await
            .map_err(map_service_error)?;
        Ok(conversation.into())
    }

    /// Report a message to the workspace moderators, or in the direct space to the instance
    /// operators. Any member who can read it, once per message (`CONFLICT` on a second
    /// report). Audited (`chat.report_message`).
    async fn report_message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        message_id: ID,
        reason: String,
    ) -> Result<MessageReport> {
        let actor = require_actor(ctx)?;
        let report = moderation(ctx)?
            .report_message(ReportMessageCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                conversation_id: parse_uuid_id(&conversation_id)?,
                message_id: parse_uuid_id(&message_id)?,
                reason,
            })
            .await
            .map_err(map_service_error)?;
        Ok(report.into())
    }

    /// Close an open report as resolved or dismissed (Owner/Moderator; for the direct space,
    /// an instance operator). Audited (`admin.resolve_report`).
    async fn resolve_report(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        report_id: ID,
        outcome: ReportOutcome,
    ) -> Result<MessageReport> {
        let actor = require_actor(ctx)?;
        let report = moderation(ctx)?
            .resolve_report(ResolveReportCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                report_id: parse_uuid_id(&report_id)?,
                outcome: outcome.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(report.into())
    }
}
