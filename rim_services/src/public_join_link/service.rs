use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::public_join_link::model::JoinRequestId;
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;

use crate::public_join_link::dto::{
    JoinByPublicLinkResultDto, JoinRequestDto, JoinRequestPageCursor, PublicJoinLinkDto,
    PublicJoinLinkPatch, PublicJoinLinkPreviewDto,
};
use crate::{Actor, ServiceResult};

/// One page of a workspace's open join requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListJoinRequestsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub dir: PageDir,
    pub cursor: Option<JoinRequestPageCursor>,
    /// Already clamped by the caller.
    pub limit: u32,
}

/// A workspace's public join link, which lets accounts of this server join or ask to join, and
/// the join requests filed through it. The link never creates an account.
///
/// Every invalid link (unknown, switched off, replaced, its workspace gone) answers `NotFound`
/// on the same path; an address that makes too many such lookups gets `RateLimited` for every
/// link lookup and join for a while.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PublicJoinLinkService: Send + Sync {
    /// The workspace's link, its settings and how it is used, for its Owner.
    ///
    /// # Errors
    /// `Validation` for the direct space and the server's default workspace, `PermissionDenied`
    /// unless the actor is the Owner.
    async fn public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<PublicJoinLinkDto>;

    /// Change the link's settings; turning it on for the first time issues its token.
    ///
    /// # Errors
    /// As [`Self::public_join_link`]; `Validation` for a role other than `Member` or `Guest`.
    async fn update_public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
        patch: PublicJoinLinkPatch,
    ) -> ServiceResult<PublicJoinLinkDto>;

    /// Issue a new token; the old one stops working at once.
    ///
    /// # Errors
    /// As [`Self::public_join_link`].
    async fn reissue_public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<PublicJoinLinkDto>;

    /// What a valid, switched-on link shows, open without signing in; for a signed-in `actor`
    /// also where they stand with the workspace.
    ///
    /// # Errors
    /// `NotFound` for every invalid link, `RateLimited` while `remote_ip` is cut off.
    async fn preview(
        &self,
        actor: Option<Actor>,
        token: String,
        remote_ip: String,
    ) -> ServiceResult<PublicJoinLinkPreviewDto>;

    /// Join through the link, or file a join request with `message` when the link takes
    /// requests.
    ///
    /// # Errors
    /// `NotFound` for every invalid link, `RateLimited` while `remote_ip` is cut off, `Forbidden`
    /// while the actor is banned, `Validation` when the workspace is full, the message too long,
    /// the actor holds too many open requests or was declined less than a day ago, `Conflict`
    /// when they are already a member or have a request open.
    async fn join(
        &self,
        actor: Actor,
        token: String,
        message: Option<String>,
        remote_ip: String,
    ) -> ServiceResult<JoinByPublicLinkResultDto>;

    /// The actor's open join requests, newest first.
    async fn my_join_requests(&self, actor: Actor) -> ServiceResult<Vec<JoinRequestDto>>;

    /// Take back the actor's open request.
    ///
    /// # Errors
    /// `NotFound` unless the request is the actor's, `Conflict` when it is no longer open.
    async fn withdraw_join_request(
        &self,
        actor: Actor,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto>;

    /// One page of the workspace's open requests, newest first, for its Owner and Moderators.
    ///
    /// # Errors
    /// `PermissionDenied` for anyone else.
    async fn list_join_requests(
        &self,
        query: ListJoinRequestsQuery,
    ) -> ServiceResult<Page<JoinRequestDto, JoinRequestPageCursor>>;

    /// Accept an open request: the person joins with the link's role, after the checks of a
    /// direct join.
    ///
    /// # Errors
    /// `PermissionDenied` unless the actor is the Owner or a Moderator, `NotFound` for a request
    /// of another workspace, `Conflict` when it is no longer open or the person is a member,
    /// `Forbidden` while they are banned, `Validation` when the workspace is full.
    async fn accept_join_request(
        &self,
        actor: Actor,
        server_id: ServerId,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto>;

    /// Decline an open request; the person may ask again a day later.
    ///
    /// # Errors
    /// As [`Self::accept_join_request`], without the join checks.
    async fn decline_join_request(
        &self,
        actor: Actor,
        server_id: ServerId,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto>;
}
