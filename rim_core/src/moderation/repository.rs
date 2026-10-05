use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::moderation::model::{
    MemberTimeout, MessageReport, MessageReportId, MessageReportStatus, NewMessageReport, ServerBan,
};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// Cursor for [`ModerationRepository::list_reports`]: the standard `(created_at, id)` tuple,
/// oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageReportCursor {
    pub created_at: DateTime<Utc>,
    pub report_id: MessageReportId,
}

/// Bans, timeouts and message reports (Track 1.2). Authorization lives in the service layer.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ModerationRepository: Send + Sync {
    /// Insert or replace the ban row for `(server_id, user_id)`.
    async fn upsert_ban(&self, ban: ServerBan) -> RepoResult<ServerBan>;

    /// The ban for `(server_id, user_id)` if it is active at `now` (no expiry, or expiry after
    /// `now`).
    async fn get_active_ban(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<ServerBan>>;

    /// Delete the ban row. Returns whether a row existed.
    async fn delete_ban(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;

    /// Bans of a workspace that are active at `now`, newest first.
    async fn list_active_bans(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<ServerBan>>;

    /// Insert or replace the timeout row for `(server_id, user_id)`.
    async fn upsert_timeout(&self, timeout: MemberTimeout) -> RepoResult<MemberTimeout>;

    /// The timeout for `(server_id, user_id)` if `until > now`.
    async fn get_active_timeout(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<MemberTimeout>>;

    /// Delete the timeout row. Returns whether a row existed (expired or not).
    async fn delete_timeout(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;

    /// Timeouts of a workspace with `until > now`.
    async fn list_active_timeouts(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<MemberTimeout>>;

    /// Insert a report. A second report of the same message by the same reporter fails with
    /// `RepoErrorKind::Conflict` (unique index).
    async fn create_report(&self, report: NewMessageReport) -> RepoResult<MessageReport>;

    async fn get_report(&self, id: MessageReportId) -> RepoResult<Option<MessageReport>>;

    /// Reports of a workspace, optionally filtered by status, ordered `created_at ASC, id ASC`
    /// with the standard cursor semantics (`docs/pagination.md`).
    async fn list_reports(
        &self,
        server_id: ServerId,
        status: Option<MessageReportStatus>,
        limit: u32,
        dir: PageDir,
        cursor: Option<MessageReportCursor>,
    ) -> RepoResult<Page<MessageReport, MessageReportCursor>>;

    /// Move an `Open` report to `status` (`Resolved` or `Dismissed`). Fails with
    /// `RepoErrorKind::PreconditionFailed` when the report is no longer open, and `NotFound`
    /// when it does not exist.
    async fn resolve_report(
        &self,
        id: MessageReportId,
        status: MessageReportStatus,
        resolved_by: UserId,
        resolved_at: DateTime<Utc>,
    ) -> RepoResult<MessageReport>;
}

#[cfg(any(test, feature = "mocks"))]
impl MockModerationRepository {
    /// A mock with no bans and no timeouts anywhere, for tests of services that consult
    /// moderation state but are not about it.
    pub fn permissive() -> Self {
        let mut mock = Self::new();
        mock.expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        mock.expect_get_active_timeout()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        mock.expect_list_active_timeouts()
            .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
        mock.expect_list_active_bans()
            .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
        mock
    }
}
