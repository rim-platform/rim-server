use async_graphql::{ComplexObject, Context, Enum, InputObject, Result, SimpleObject};
use rim_core::public_join_link::model::{JoinRequestStatus, PublicLinkMode};
use rim_services::public_join_link::dto::{
    JoinByPublicLinkOutcome, JoinByPublicLinkResultDto, JoinRequestDto, JoinRequestPageCursor,
    PublicJoinLinkDto, PublicJoinLinkPreviewDto, PublicLinkViewer,
};

use super::{PageInfo, ServerMemberRoleGql, User};
use crate::ids::parse_uuid_id;
use crate::resolve::ResolveExt;

/// What opening a workspace's public link does.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PublicJoinLinkMode {
    /// The person joins at once.
    Direct,
    /// The person asks to join; the Owner or a Moderator accepts or declines.
    Requests,
}

impl From<PublicLinkMode> for PublicJoinLinkMode {
    fn from(mode: PublicLinkMode) -> Self {
        match mode {
            PublicLinkMode::Direct => Self::Direct,
            PublicLinkMode::Requests => Self::Requests,
        }
    }
}

impl From<PublicJoinLinkMode> for PublicLinkMode {
    fn from(mode: PublicJoinLinkMode) -> Self {
        match mode {
            PublicJoinLinkMode::Direct => Self::Direct,
            PublicJoinLinkMode::Requests => Self::Requests,
        }
    }
}

/// A workspace's public link as its Owner sees it: the settings, the link itself, which the
/// Owner can copy again at any time, and how it is used.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct PublicJoinLink {
    pub server_id: async_graphql::ID,
    /// Whether the link admits anyone; off by default.
    pub enabled: bool,
    pub mode: PublicJoinLinkMode,
    /// `MEMBER` or `GUEST`: the role a person joining through the link gets.
    pub role: ServerMemberRoleGql,
    /// Whether accounts from other servers may join through the link.
    pub allow_remote: bool,
    /// When the link was last turned on.
    pub enabled_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the current token was issued; null while the workspace never had a link.
    pub issued_at: Option<chrono::DateTime<chrono::Utc>>,
    pub issued_by_id: Option<async_graphql::ID>,
    /// The token, `rim_pjl_` and 22 base64url characters; null while the workspace never had a
    /// link.
    pub token: Option<String>,
    /// `<server>/join/<token>`; null while the workspace never had a link.
    pub url: Option<String>,
    /// Joins through the link, directly or by an accepted request, in the last 24 hours.
    pub joins_last_day: i32,
    /// Joins through the link in the last seven days.
    pub joins_last_week: i32,
    /// The workspace's open join requests.
    pub open_requests: i32,
}

#[ComplexObject]
impl PublicJoinLink {
    /// Who issued the current token (null once the account no longer exists).
    async fn issued_by(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match &self.issued_by_id {
            Some(id) => ctx.load_user(id).await,
            None => Ok(None),
        }
    }
}

fn count(n: u64) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

impl From<PublicJoinLinkDto> for PublicJoinLink {
    fn from(dto: PublicJoinLinkDto) -> Self {
        Self {
            server_id: dto.server_id.to_string().into(),
            enabled: dto.enabled,
            mode: dto.mode.into(),
            role: dto.role.into(),
            allow_remote: dto.allow_remote,
            enabled_at: dto.enabled_at,
            issued_at: dto.issued_at,
            issued_by_id: dto.issued_by.map(|id| id.to_string().into()),
            token: dto.token,
            url: dto.url,
            joins_last_day: count(dto.joins_last_day),
            joins_last_week: count(dto.joins_last_week),
            open_requests: count(dto.open_requests),
        }
    }
}

/// The Owner's change to a workspace's public link; an omitted field stays as it is. Turning
/// the link on for the first time issues its token.
#[derive(InputObject, Clone, Debug)]
pub struct UpdatePublicJoinLinkInput {
    pub server_id: async_graphql::ID,
    pub enabled: Option<bool>,
    pub mode: Option<PublicJoinLinkMode>,
    /// `MEMBER` or `GUEST`; any other role is `VALIDATION`.
    pub role: Option<ServerMemberRoleGql>,
    pub allow_remote: Option<bool>,
}

/// Where the caller of `publicJoinLinkPreview` stands with the link's workspace.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PublicJoinLinkViewer {
    /// Nobody is signed in.
    SignedOut,
    /// Signed in, not a member, not banned, no open request.
    NotMember,
    Member,
    /// Banned from the workspace: cannot join or ask to.
    Banned,
    /// The caller's join request is waiting for the moderators.
    RequestOpen,
}

impl From<PublicLinkViewer> for PublicJoinLinkViewer {
    fn from(viewer: PublicLinkViewer) -> Self {
        match viewer {
            PublicLinkViewer::SignedOut => Self::SignedOut,
            PublicLinkViewer::NotMember => Self::NotMember,
            PublicLinkViewer::Member => Self::Member,
            PublicLinkViewer::Banned => Self::Banned,
            PublicLinkViewer::RequestOpen => Self::RequestOpen,
        }
    }
}

/// What a valid, switched-on public link shows anyone who has it.
#[derive(SimpleObject, Clone, Debug)]
pub struct PublicJoinLinkPreview {
    pub server_id: async_graphql::ID,
    pub name: String,
    pub mode: PublicJoinLinkMode,
    pub viewer: PublicJoinLinkViewer,
    /// The caller's open join request, with `REQUEST_OPEN`.
    pub join_request_id: Option<async_graphql::ID>,
}

impl From<PublicJoinLinkPreviewDto> for PublicJoinLinkPreview {
    fn from(dto: PublicJoinLinkPreviewDto) -> Self {
        Self {
            server_id: dto.server_id.to_string().into(),
            name: dto.name,
            mode: dto.mode.into(),
            viewer: dto.viewer.into(),
            join_request_id: dto.join_request_id.map(|id| id.to_string().into()),
        }
    }
}

/// Where a join request stands.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(name = "JoinRequestStatus")]
pub enum JoinRequestStatusGql {
    Open,
    Accepted,
    Declined,
    /// The person took it back.
    Withdrawn,
    /// A ban or the person's account deletion ended it.
    Cancelled,
    /// It stayed open for 30 days.
    Expired,
}

impl From<JoinRequestStatus> for JoinRequestStatusGql {
    fn from(status: JoinRequestStatus) -> Self {
        match status {
            JoinRequestStatus::Open => Self::Open,
            JoinRequestStatus::Accepted => Self::Accepted,
            JoinRequestStatus::Declined => Self::Declined,
            JoinRequestStatus::Withdrawn => Self::Withdrawn,
            JoinRequestStatus::Cancelled => Self::Cancelled,
            JoinRequestStatus::Expired => Self::Expired,
        }
    }
}

/// A person's request to join a workspace through its public link.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct JoinRequest {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub workspace_name: String,
    pub user_id: async_graphql::ID,
    /// The person's note to the moderators, at most 500 characters.
    pub message: Option<String>,
    pub status: JoinRequestStatusGql,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When an open request expires: 30 days after it was filed.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    pub decided_by_id: Option<async_graphql::ID>,
}

#[ComplexObject]
impl JoinRequest {
    /// The person asking (null once the account no longer exists).
    async fn user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.user_id).await
    }
}

impl From<JoinRequestDto> for JoinRequest {
    fn from(dto: JoinRequestDto) -> Self {
        Self {
            id: dto.id.to_string().into(),
            server_id: dto.server_id.to_string().into(),
            workspace_name: dto.workspace_name,
            user_id: dto.user_id.to_string().into(),
            message: dto.message,
            status: dto.status.into(),
            created_at: dto.created_at,
            expires_at: dto.expires_at,
            decided_at: dto.decided_at,
            decided_by_id: dto.decided_by.map(|id| id.to_string().into()),
        }
    }
}

/// What `joinByPublicLink` did.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(name = "JoinByPublicLinkOutcome")]
pub enum JoinByPublicLinkOutcomeGql {
    /// The caller is a member now.
    Joined,
    /// The caller's join request was filed.
    Requested,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct JoinByPublicLinkPayload {
    pub outcome: JoinByPublicLinkOutcomeGql,
    pub server_id: async_graphql::ID,
    /// The filed request, with `REQUESTED`.
    pub join_request: Option<JoinRequest>,
}

impl From<JoinByPublicLinkResultDto> for JoinByPublicLinkPayload {
    fn from(dto: JoinByPublicLinkResultDto) -> Self {
        Self {
            outcome: match dto.outcome {
                JoinByPublicLinkOutcome::Joined => JoinByPublicLinkOutcomeGql::Joined,
                JoinByPublicLinkOutcome::Requested => JoinByPublicLinkOutcomeGql::Requested,
            },
            server_id: dto.server_id.to_string().into(),
            join_request: dto.join_request.map(Into::into),
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct JoinRequestCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub request_id: async_graphql::ID,
}

impl TryFrom<JoinRequestCursorInput> for JoinRequestPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: JoinRequestCursorInput) -> Result<Self> {
        Ok(Self {
            created_at: value.created_at,
            request_id: parse_uuid_id(&value.request_id)?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct JoinRequestCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub request_id: async_graphql::ID,
}

impl From<JoinRequestPageCursor> for JoinRequestCursorOutput {
    fn from(value: JoinRequestPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            request_id: value.request_id.to_string().into(),
        }
    }
}

/// A workspace's open join requests ordered `createdAt DESC, id DESC` (newest first), with the
/// standard cursor semantics: `AFTER` pages go to older requests.
#[derive(SimpleObject, Clone, Debug)]
pub struct JoinRequestPage {
    pub items: Vec<JoinRequest>,
    pub next: Option<JoinRequestCursorOutput>,
    pub prev: Option<JoinRequestCursorOutput>,
    pub page_info: PageInfo,
}

impl JoinRequestPage {
    pub fn from_page(
        page: rim_core::repository::pagination::Page<JoinRequestDto, JoinRequestPageCursor>,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(Into::into),
            prev: page.prev.map(Into::into),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}
