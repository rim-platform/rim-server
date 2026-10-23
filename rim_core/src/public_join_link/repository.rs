use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::public_join_link::model::{
    JoinRequest, JoinRequestId, JoinRequestStatus, NewJoinRequest, PublicJoinLink,
};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// Cursor for [`PublicJoinLinkRepository::list_open_requests`]: the standard
/// `(created_at, id)` tuple, newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinRequestCursor {
    pub created_at: DateTime<Utc>,
    pub request_id: JoinRequestId,
}

/// Parameters of [`PublicJoinLinkRepository::list_open_requests`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListJoinRequestsParams {
    pub server_id: ServerId,
    /// Requests whose expiry is not after this are left out.
    pub now: DateTime<Utc>,
    pub limit: u32,
    pub dir: PageDir,
    pub cursor: Option<JoinRequestCursor>,
}

/// Workspaces' public links, the joins made through them, and join requests.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PublicJoinLinkRepository: Send + Sync {
    /// The workspace's link, whether on or off; `None` when it never had one.
    async fn link(&self, server_id: ServerId) -> RepoResult<Option<PublicJoinLink>>;

    /// The link whose token hashes to `token_hash`, whether on or off.
    async fn link_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PublicJoinLink>>;

    /// Store `link` as its workspace's link, replacing the one there. Returns it as stored.
    async fn save_link(&self, link: PublicJoinLink) -> RepoResult<PublicJoinLink>;

    /// How many links hold a sealed token.
    async fn count_links(&self) -> RepoResult<u64>;

    /// Record that `user_id` joined `server_id` through its link at `at`, and delete the
    /// workspace's join records older than seven days before `at`.
    async fn record_join(
        &self,
        server_id: ServerId,
        user_id: UserId,
        at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Joins through the workspace's link at or after `since`.
    async fn count_joins_since(&self, server_id: ServerId, since: DateTime<Utc>)
    -> RepoResult<u64>;

    /// File a join request. A stored open request of the same person and workspace that expired
    /// by `request.created_at` is marked expired first.
    ///
    /// # Errors
    /// `Conflict` when the person already has an open request for the workspace.
    async fn create_request(&self, request: NewJoinRequest) -> RepoResult<JoinRequest>;

    /// The request with `id`, whatever its status.
    async fn request(&self, id: JoinRequestId) -> RepoResult<Option<JoinRequest>>;

    /// The person's request for the workspace that is open at `now`.
    async fn open_request(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<JoinRequest>>;

    /// When the person's latest declined request for the workspace was declined.
    async fn last_declined_at(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<DateTime<Utc>>>;

    /// The person's requests open at `now`, across workspaces.
    async fn count_open_for_user(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<u64>;

    /// The workspace's requests open at `now`.
    async fn count_open_for_server(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// One page of the workspace's requests open at `params.now`, newest first
    /// (`created_at DESC, id DESC`) with the standard cursor rules.
    async fn list_open_requests(
        &self,
        params: ListJoinRequestsParams,
    ) -> RepoResult<Page<JoinRequest, JoinRequestCursor>>;

    /// The person's requests open at `now`, newest first.
    async fn list_open_for_user(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<JoinRequest>>;

    /// Close the request with `status`, decided by `decided_by` at `now`, when it is open at
    /// `now`. Returns the closed request, or `None` when it was not open.
    async fn close_request(
        &self,
        id: JoinRequestId,
        status: JoinRequestStatus,
        decided_by: Option<UserId>,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<JoinRequest>>;

    /// Cancel the person's requests for the workspace that are open at `now`. Returns them.
    async fn cancel_open_requests(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<JoinRequest>>;
}
