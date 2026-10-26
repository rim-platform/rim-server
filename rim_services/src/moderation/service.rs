use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::ServiceResult;
use crate::conversation::dto::ConversationDto;
use crate::moderation::command::{
    AuthorizeReportAttachmentQuery, BanMemberCmd, ClearTimeoutCmd, GetReportQuery, KickMemberCmd,
    ListBansQuery, ListReportsQuery, ReportMessageCmd, ResolveReportCmd, SetChannelPostingCmd,
    SetMemberRoleCmd, SetSlowModeCmd, TimeoutMemberCmd, UnbanMemberCmd,
};
use crate::moderation::dto::{
    MemberTimeoutDto, MessageReportDto, ReportAttachmentDto, ReportPageCursor, ServerBanDto,
};
use crate::server_member::dto::ServerMemberDto;

/// Roles and moderation. Every
/// mutating method writes a `command_audit_log` row under a stable pseudo command id
/// (`admin.kick`, `admin.ban`, ...), whether it was called from GraphQL or a command.
///
/// Errors common to all methods: `PermissionDenied` when the actor lacks the role or targets
/// someone they may not act on, `NotFound` for an unknown target, `Validation` for bad input.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ModerationService: Send + Sync {
    async fn set_member_role(&self, cmd: SetMemberRoleCmd) -> ServiceResult<ServerMemberDto>;

    async fn kick(&self, cmd: KickMemberCmd) -> ServiceResult<()>;

    async fn ban(&self, cmd: BanMemberCmd) -> ServiceResult<ServerBanDto>;

    /// Returns whether a ban existed.
    async fn unban(&self, cmd: UnbanMemberCmd) -> ServiceResult<bool>;

    /// A user's active ban, visible to workspace members; its reason only to moderators.
    async fn member_ban(
        &self,
        actor: crate::Actor,
        server_id: ServerId,
        user_id: UserId,
    ) -> ServiceResult<Option<ServerBanDto>>;

    async fn list_bans(&self, query: ListBansQuery) -> ServiceResult<Vec<ServerBanDto>>;

    async fn timeout(&self, cmd: TimeoutMemberCmd) -> ServiceResult<MemberTimeoutDto>;

    /// Returns whether a timeout was in force.
    async fn clear_timeout(&self, cmd: ClearTimeoutCmd) -> ServiceResult<bool>;

    async fn set_slow_mode(&self, cmd: SetSlowModeCmd) -> ServiceResult<ConversationDto>;

    /// Sets a channel's posting policy; returns the channel. Unchanged settings write nothing.
    async fn set_channel_posting(
        &self,
        cmd: SetChannelPostingCmd,
    ) -> ServiceResult<ConversationDto>;

    async fn report_message(&self, cmd: ReportMessageCmd) -> ServiceResult<MessageReportDto>;

    async fn list_reports(
        &self,
        query: ListReportsQuery,
    ) -> ServiceResult<Page<MessageReportDto, ReportPageCursor>>;

    async fn resolve_report(&self, cmd: ResolveReportCmd) -> ServiceResult<MessageReportDto>;

    /// One report with its verified content, if any. Records `e2ee.report_viewed` when the
    /// content is returned.
    async fn get_report(&self, query: GetReportQuery) -> ServiceResult<MessageReportDto>;

    /// The stored blob of a reported encrypted attachment. `NotFound` for every refusal, so a
    /// caller learns nothing about reports it may not see.
    async fn authorize_report_attachment(
        &self,
        query: AuthorizeReportAttachmentQuery,
    ) -> ServiceResult<ReportAttachmentDto>;

    /// Internal gate (no actor): the end of `user_id`'s timeout in `server_id` while one is in
    /// force. Used by the command service to refuse Mutating commands.
    async fn active_timeout(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> ServiceResult<Option<DateTime<Utc>>>;
}

#[cfg(any(test, feature = "mocks"))]
impl MockModerationService {
    /// A mock that reports no active timeout for anyone, for tests of the command service that
    /// are not about the timeout gate.
    pub fn no_timeouts() -> Self {
        let mut mock = Self::new();
        mock.expect_active_timeout()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock
    }
}
