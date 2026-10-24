use async_graphql::{ComplexObject, Context, Enum, InputObject, Result, SimpleObject};

use super::{PageInfo, ServerMemberRoleGql, User};
use crate::ids::parse_uuid_id;
use crate::resolve::ResolveExt;

// ── Registration types ─────────────────────────────────────────────────────

/// How new accounts may sign up on this instance.
#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RegistrationModeGql {
    /// Default: signup requires a valid token (instance or workspace invite).
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
    /// `RIM_REGISTRATION_POW_BITS`: `signupWithKey` takes a token or a proof-of-work whose
    /// difficulty the discovery document's `registration.powBits` gives; a password signup
    /// takes a token.
    Pow,
}

impl From<rim_services::auth::dto::RegistrationMode> for RegistrationModeGql {
    fn from(m: rim_services::auth::dto::RegistrationMode) -> Self {
        use rim_services::auth::dto::RegistrationMode as D;
        match m {
            D::InviteOnly => Self::InviteOnly,
            D::Open => Self::Open,
            D::Pow { .. } => Self::Pow,
        }
    }
}

/// The type of an invite token, used by `InviteTokenPreview`.
#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum InviteTokenTypeGql {
    /// Instance invite: routes to `instanceInviteToken` input on `signupWithInvite`.
    Instance,
    /// Workspace invite: join-a-workspace flow.
    Workspace,
}

impl From<rim_services::auth::dto::InviteTokenType> for InviteTokenTypeGql {
    fn from(t: rim_services::auth::dto::InviteTokenType) -> Self {
        use rim_services::auth::dto::InviteTokenType as D;
        match t {
            D::Instance => Self::Instance,
            D::Workspace => Self::Workspace,
        }
    }
}

/// Minimal workspace info included in a workspace-invite preview. The workspace name and the
/// inviter's and owner's display names are shown to anyone holding the invite link.
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceInvitePreviewGql {
    pub server_id: async_graphql::ID,
    pub server_name: String,
    pub invited_by_display_name: String,
    /// The workspace owner's display name, else username; null when the workspace has no owner.
    pub owner_display_name: Option<String>,
}

impl From<rim_services::auth::dto::WorkspaceInvitePreviewDto> for WorkspaceInvitePreviewGql {
    fn from(dto: rim_services::auth::dto::WorkspaceInvitePreviewDto) -> Self {
        Self {
            server_id: dto.server_id.to_string().into(),
            server_name: dto.server_name,
            invited_by_display_name: dto.invited_by_display_name,
            owner_display_name: dto.owner_display_name,
        }
    }
}

/// Read-only preview of a registration token.
///
/// Does NOT consume the token. Workspace-invite previews carry server and
/// inviter info; instance-invite previews are minimal.
#[derive(SimpleObject, Clone, Debug)]
pub struct InviteTokenPreviewGql {
    pub token_type: InviteTokenTypeGql,
    /// `None` for workspace invites that were issued without an expiry.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Populated only when `token_type == WORKSPACE`.
    pub workspace: Option<WorkspaceInvitePreviewGql>,
}

impl From<rim_services::auth::dto::InviteTokenPreviewDto> for InviteTokenPreviewGql {
    fn from(dto: rim_services::auth::dto::InviteTokenPreviewDto) -> Self {
        Self {
            token_type: dto.token_type.into(),
            expires_at: dto.expires_at,
            workspace: dto.workspace.map(Into::into),
        }
    }
}

/// Payload returned from `signupWithInvite` and `signupWithKey`.
///
/// `server_id` is `Some` only when a `workspaceInviteToken` was used;
/// the instance-invite and open paths land in "no workspaces yet" state.
#[derive(SimpleObject, Clone, Debug)]
pub struct SignupWithInvitePayloadGql {
    pub user_id: async_graphql::ID,
    pub server_id: Option<async_graphql::ID>,
    /// The new session's CSRF token, the value of the `rim_csrf` cookie. A client that cannot
    /// read that cookie keeps it in memory and sends it in the `X-CSRF-Token` header.
    pub csrf_token: String,
    /// After `signupWithKey`: the new account's server share, unpadded base64url, in the
    /// clear. `null` for the other signups.
    pub server_share: Option<String>,
    /// After `signupWithKey` without `keepSignedIn`: the session secret, unpadded base64url, in
    /// the clear, as `loginWithKey` returns it. `null` for a kept session.
    pub session_secret: Option<String>,
    /// After `signupWithKey` with `keepSignedIn` and without `keepKeyHeld`: the keep key the
    /// server holds for this session, unpadded base64url, in the clear. `null` otherwise.
    pub keep_key: Option<String>,
}

// ── Workspace invites ──────────────────────────────────────────────────────

/// Whether a workspace invite still admits anyone. A revocation wins over an expiry, and an
/// expiry over the uses running out.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkspaceInviteState {
    /// Unrevoked, unexpired and with uses left.
    Active,
    /// Past `expiresAt`.
    Expired,
    /// `useCount` reached `maxUses`.
    UsedUp,
    /// Revoked by an owner (`revokeInvite`), or by the instance when its creator was blocked
    /// or deleted.
    Revoked,
}

impl From<rim_services::auth::dto::InviteState> for WorkspaceInviteState {
    fn from(state: rim_services::auth::dto::InviteState) -> Self {
        use rim_services::auth::dto::InviteState as D;
        match state {
            D::Active => Self::Active,
            D::Expired => Self::Expired,
            D::UsedUp => Self::UsedUp,
            D::Revoked => Self::Revoked,
        }
    }
}

/// A workspace invite as its workspace's owner sees it. The token is shown once, by
/// `createInvite`, and never again.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct WorkspaceInvite {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    /// The role a person joining with the invite gets.
    pub role: ServerMemberRoleGql,
    pub use_count: i32,
    pub max_uses: i32,
    /// Null for an invite stored without an expiry, which does not expire.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by_id: async_graphql::ID,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The invite's state when it was read.
    pub state: WorkspaceInviteState,
}

#[ComplexObject]
impl WorkspaceInvite {
    /// The member who created the invite (null if the account no longer exists).
    async fn created_by(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.created_by_id).await
    }
}

impl From<rim_services::auth::dto::InviteDto> for WorkspaceInvite {
    fn from(dto: rim_services::auth::dto::InviteDto) -> Self {
        let state = dto.state_at(chrono::Utc::now()).into();
        Self {
            id: dto.id.to_string().into(),
            server_id: dto.server_id.to_string().into(),
            role: dto.role.into(),
            use_count: dto.use_count,
            max_uses: dto.max_uses,
            expires_at: dto.expires_at,
            created_at: dto.created_at,
            created_by_id: dto.created_by_user_id.to_string().into(),
            revoked_at: dto.revoked_at,
            state,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct WorkspaceInviteCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub invite_id: async_graphql::ID,
}

impl TryFrom<WorkspaceInviteCursorInput> for rim_services::auth::dto::InvitePageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: WorkspaceInviteCursorInput) -> Result<Self> {
        Ok(Self {
            created_at: value.created_at,
            invite_id: parse_uuid_id::<rim_core::auth::model::InviteId>(&value.invite_id)?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceInviteCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub invite_id: async_graphql::ID,
}

impl From<rim_services::auth::dto::InvitePageCursor> for WorkspaceInviteCursorOutput {
    fn from(value: rim_services::auth::dto::InvitePageCursor) -> Self {
        Self {
            created_at: value.created_at,
            invite_id: async_graphql::ID(value.invite_id.to_string()),
        }
    }
}

/// Workspace invites ordered `createdAt DESC, id DESC` (newest first), with the standard cursor
/// semantics: `AFTER` pages go to older invites.
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceInvitePage {
    pub items: Vec<WorkspaceInvite>,
    pub next: Option<WorkspaceInviteCursorOutput>,
    pub prev: Option<WorkspaceInviteCursorOutput>,
    pub page_info: PageInfo,
}

impl WorkspaceInvitePage {
    pub fn from_page(
        page: rim_core::repository::pagination::Page<
            rim_services::auth::dto::InviteDto,
            rim_services::auth::dto::InvitePageCursor,
        >,
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
