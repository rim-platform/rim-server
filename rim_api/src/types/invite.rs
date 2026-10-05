use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone, Debug)]
pub struct Invite {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub created_by_user_id: async_graphql::ID,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::auth::dto::InviteDto> for Invite {
    fn from(value: rim_services::auth::dto::InviteDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            created_by_user_id: async_graphql::ID(value.created_by_user_id.to_string()),
            expires_at: value.expires_at,
            max_uses: value.max_uses,
            use_count: value.use_count,
            revoked_at: value.revoked_at,
        }
    }
}

// ── Registration types ─────────────────────────────────────────────────────

/// GraphQL enum for the deployment-wide registration mode.
///
/// Mirrors [`rim_services::auth::dto::RegistrationMode`].
/// Variants appear in the SDL as `INVITE_ONLY` and `OPEN`.
#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RegistrationModeGql {
    /// Default: signup requires a valid token (bootstrap, instance, or workspace invite).
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
}

impl From<rim_services::auth::dto::RegistrationMode> for RegistrationModeGql {
    fn from(m: rim_services::auth::dto::RegistrationMode) -> Self {
        use rim_services::auth::dto::RegistrationMode as D;
        match m {
            D::InviteOnly => Self::InviteOnly,
            D::Open => Self::Open,
        }
    }
}

/// GraphQL enum for the type of an invite token, used by `InviteTokenPreview`.
///
/// `BOOTSTRAP` and `INSTANCE` both trigger the "create your RIM account" form
/// copy on the frontend, but they are distinct so the frontend knows which input
/// field to populate on `signupWithInvite`:
/// - `BOOTSTRAP` → use `bootstrapToken`
/// - `INSTANCE`  → use `instanceInviteToken`
///
/// Mirrors [`rim_services::auth::dto::InviteTokenType`].
#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum InviteTokenTypeGql {
    /// Bootstrap token: routes to `bootstrapToken` input on `signupWithInvite`.
    Bootstrap,
    /// Instance invite: routes to `instanceInviteToken` input on `signupWithInvite`.
    Instance,
    /// Workspace invite: join-a-workspace flow.
    Workspace,
}

impl From<rim_services::auth::dto::InviteTokenType> for InviteTokenTypeGql {
    fn from(t: rim_services::auth::dto::InviteTokenType) -> Self {
        use rim_services::auth::dto::InviteTokenType as D;
        match t {
            D::Bootstrap => Self::Bootstrap,
            D::Instance => Self::Instance,
            D::Workspace => Self::Workspace,
        }
    }
}

/// Minimal workspace info included in a workspace-invite preview.
///
/// See spec §4.7 — inviter name and server name are disclosed because
/// the user was already shown them on the invite-link landing page.
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
/// inviter info; instance/bootstrap previews are minimal (§4.7).
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

/// Output type for an instance invite (admin view).
///
/// Mirrors the `InstanceInvite` SDL type in spec §9.2.
/// Note: the raw invite token is **not** exposed here — it is only emitted
/// once in `CreateInstanceInvitePayload.inviteToken`.
#[derive(SimpleObject, Clone, Debug)]
pub struct InstanceInviteGql {
    pub id: async_graphql::ID,
    pub created_by_user_id: async_graphql::ID,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::auth::dto::InstanceInviteDto> for InstanceInviteGql {
    fn from(dto: rim_services::auth::dto::InstanceInviteDto) -> Self {
        Self {
            id: dto.id.to_string().into(),
            created_by_user_id: dto.created_by_user_id.to_string().into(),
            expires_at: dto.expires_at,
            max_uses: dto.max_uses,
            use_count: dto.use_count,
            revoked_at: dto.revoked_at,
            created_at: dto.created_at,
        }
    }
}

/// Payload returned after successfully creating an instance invite.
///
/// `invite_token` is the raw (unhashed) token — shown to the admin exactly once.
#[derive(SimpleObject, Clone, Debug)]
pub struct CreateInstanceInvitePayloadGql {
    /// The raw invite token. Shown to the admin once; never stored unhashed.
    pub invite_token: String,
    pub invite: InstanceInviteGql,
}

/// Payload returned from `signupWithInvite`.
///
/// `server_id` is `Some` only when a `workspaceInviteToken` was used;
/// bootstrap and instance-invite paths land in "no workspaces yet" state.
#[derive(SimpleObject, Clone, Debug)]
pub struct SignupWithInvitePayloadGql {
    pub user_id: async_graphql::ID,
    pub server_id: Option<async_graphql::ID>,
}
