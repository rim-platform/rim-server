use async_graphql::SimpleObject;

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
/// inviter's display name are shown to anyone holding the invite link.
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceInvitePreviewGql {
    pub server_id: async_graphql::ID,
    pub server_name: String,
    pub invited_by_display_name: String,
}

impl From<rim_services::auth::dto::WorkspaceInvitePreviewDto> for WorkspaceInvitePreviewGql {
    fn from(dto: rim_services::auth::dto::WorkspaceInvitePreviewDto) -> Self {
        Self {
            server_id: dto.server_id.to_string().into(),
            server_name: dto.server_name,
            invited_by_display_name: dto.invited_by_display_name,
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

/// Payload returned from `signupWithInvite`.
///
/// `server_id` is `Some` only when a `workspaceInviteToken` was used;
/// the instance-invite and open paths land in "no workspaces yet" state.
#[derive(SimpleObject, Clone, Debug)]
pub struct SignupWithInvitePayloadGql {
    pub user_id: async_graphql::ID,
    pub server_id: Option<async_graphql::ID>,
}
