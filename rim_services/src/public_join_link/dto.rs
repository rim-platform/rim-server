//! What the public join link service answers: the Owner's view of the link, a link's preview,
//! the outcome of a join, and join requests.

use chrono::{DateTime, Utc};

use rim_core::public_join_link::model::{
    JoinRequest, JoinRequestId, JoinRequestStatus, PublicLinkMode,
};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;

/// Longest message a join request carries, in characters.
pub const MAX_JOIN_REQUEST_MESSAGE_CHARS: usize = 500;
/// Days a join request stays open unless it is decided first.
pub const JOIN_REQUEST_DAYS: i64 = 30;
/// Most join requests one person holds open at a time, across workspaces.
pub const MAX_OPEN_JOIN_REQUESTS_PER_USER: u64 = 20;
/// Hours after a decline before the person may ask the same workspace again.
pub const JOIN_REQUEST_RETRY_HOURS: i64 = 24;
/// Failed link lookups and joins one address may make in [`LINK_MISS_WINDOW_SECS`].
pub const LINK_MISS_LIMIT: u32 = 20;
/// The window [`LINK_MISS_LIMIT`] counts in: ten minutes.
pub const LINK_MISS_WINDOW_SECS: i64 = 10 * 60;
/// How long an address that passed [`LINK_MISS_LIMIT`] gets `RateLimited` for every link
/// lookup and join: one hour.
pub const LINK_MISS_BLOCK_SECS: i64 = 60 * 60;
/// The `source` of a `MemberJoined` through a public link.
pub const PUBLIC_LINK_SOURCE: &str = "public_link";

/// The Owner's view of a workspace's public link: its settings, the link itself and how it is
/// used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicJoinLinkDto {
    pub server_id: ServerId,
    pub enabled: bool,
    pub mode: PublicLinkMode,
    /// `Member` or `Guest`.
    pub role: ServerMemberRole,
    pub allow_remote: bool,
    pub enabled_at: Option<DateTime<Utc>>,
    /// When the current token was issued; `None` while the workspace never had a link.
    pub issued_at: Option<DateTime<Utc>>,
    pub issued_by: Option<UserId>,
    /// The token; `None` while the workspace never had a link.
    pub token: Option<String>,
    /// `<link base>/join/<token>`; `None` while the workspace never had a link.
    pub url: Option<String>,
    /// Joins through the link (directly or by an accepted request) in the last 24 hours.
    pub joins_last_day: u64,
    /// Joins through the link in the last seven days.
    pub joins_last_week: u64,
    /// The workspace's open join requests.
    pub open_requests: u64,
}

/// The Owner's change to a link's settings; a `None` field stays as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublicJoinLinkPatch {
    pub enabled: Option<bool>,
    pub mode: Option<PublicLinkMode>,
    /// `Member` or `Guest`; any other role is `Validation`.
    pub role: Option<ServerMemberRole>,
    pub allow_remote: Option<bool>,
}

/// Where the caller of a preview stands with the link's workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicLinkViewer {
    /// Nobody is signed in.
    SignedOut,
    /// Signed in, not a member, not banned, no open request.
    NotMember,
    Member,
    /// An active ban: the caller cannot join or ask to.
    Banned,
    /// The caller's join request is waiting for the moderators.
    RequestOpen,
}

/// What a valid, switched-on link shows anyone who has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicJoinLinkPreviewDto {
    pub server_id: ServerId,
    pub name: String,
    pub mode: PublicLinkMode,
    pub viewer: PublicLinkViewer,
    /// The caller's open request, with [`PublicLinkViewer::RequestOpen`].
    pub join_request_id: Option<JoinRequestId>,
}

/// What a valid, switched-on link that admits accounts from other servers shows a relay
/// presenting a person of another server: the workspace and what joining does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLinkDto {
    pub server_id: ServerId,
    pub name: String,
    pub mode: PublicLinkMode,
    /// The role a person joining through the link gets.
    pub role: ServerMemberRole,
}

/// What a join through a link did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinByPublicLinkOutcome {
    /// The caller is a member now.
    Joined,
    /// The caller's join request was filed.
    Requested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinByPublicLinkResultDto {
    pub outcome: JoinByPublicLinkOutcome,
    pub server_id: ServerId,
    /// The filed request, with [`JoinByPublicLinkOutcome::Requested`].
    pub join_request: Option<JoinRequestDto>,
}

/// A join request, with the name of the workspace it asks to join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequestDto {
    pub id: JoinRequestId,
    pub server_id: ServerId,
    pub workspace_name: String,
    pub user_id: UserId,
    pub message: Option<String>,
    /// The status as of the read: an open request past its expiry is `Expired`.
    pub status: JoinRequestStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<UserId>,
}

impl JoinRequestDto {
    /// `request` of the workspace named `workspace_name`, its status as of `now`.
    pub fn from_request(request: JoinRequest, workspace_name: String, now: DateTime<Utc>) -> Self {
        Self {
            status: request.status_at(now),
            id: request.id,
            server_id: request.server_id,
            workspace_name,
            user_id: request.user_id,
            message: request.message,
            created_at: request.created_at,
            expires_at: request.expires_at,
            decided_at: request.decided_at,
            decided_by: request.decided_by,
        }
    }
}

/// The cursor of `joinRequests`: the standard `(created_at, id)` tuple, newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinRequestPageCursor {
    pub created_at: DateTime<Utc>,
    pub request_id: JoinRequestId,
}

impl From<rim_core::public_join_link::repository::JoinRequestCursor> for JoinRequestPageCursor {
    fn from(c: rim_core::public_join_link::repository::JoinRequestCursor) -> Self {
        Self {
            created_at: c.created_at,
            request_id: c.request_id,
        }
    }
}

impl From<JoinRequestPageCursor> for rim_core::public_join_link::repository::JoinRequestCursor {
    fn from(c: JoinRequestPageCursor) -> Self {
        Self {
            created_at: c.created_at,
            request_id: c.request_id,
        }
    }
}
