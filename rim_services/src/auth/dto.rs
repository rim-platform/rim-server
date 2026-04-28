use chrono::{DateTime, Utc};
use rim_core::auth::model::InstanceInviteId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AuthSessionDto {
    pub user_id: UserId,
    pub session_id: rim_core::auth::model::SessionId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InviteDto {
    pub id: rim_core::auth::model::InviteId,
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BootstrapResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LoginResultDto {
    pub user_id: UserId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateInviteResultDto {
    pub invite: InviteDto,
    pub invite_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AcceptInviteResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JoinServerByInviteResultDto {
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestAuthResultDto {
    pub user_id: Option<UserId>,
    pub session_id: Option<rim_core::auth::model::SessionId>,
}

/// Result of any signup path (bootstrap, instance-invite, open).
///
/// `server_id` is `Some` only when a workspace invite was used;
/// bootstrap and instance-invite signups land in "no workspaces yet" state.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SignupResultDto {
    pub user_id: UserId,
    pub server_id: Option<ServerId>,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

/// Metadata about an instance invite, returned to admin callers.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InstanceInviteDto {
    pub id: InstanceInviteId,
    pub created_by_user_id: UserId,
    pub expires_at: DateTime<Utc>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Result of `create_instance_invite`, containing the raw token for display.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateInstanceInviteResultDto {
    pub invite: InstanceInviteDto,
    /// The raw (unhashed) invite token. Shown to the admin exactly once.
    pub invite_token: String,
}

/// Token type discriminant for the preview query.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum InviteTokenType {
    /// Bootstrap or instance invite: "create your RIM account" copy.
    Instance,
    /// Workspace invite: "join workspace X" copy.
    Workspace,
}

/// Minimal workspace info included in a workspace-invite preview.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkspaceInvitePreviewDto {
    pub server_id: ServerId,
    pub server_name: String,
    pub invited_by_display_name: String,
}

/// Read-only preview of a registration token.
///
/// Does NOT consume the token. See spec §4.7.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InviteTokenPreviewDto {
    pub token_type: InviteTokenType,
    pub expires_at: Option<DateTime<Utc>>,
    /// Populated only when `token_type == Workspace`.
    pub workspace: Option<WorkspaceInvitePreviewDto>,
}

/// The registration mode in effect for this deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RegistrationMode {
    /// Default: signup requires a valid token (bootstrap, instance, or workspace invite).
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
}

/// Zero-sized token type used as a GraphQL context data key for the
/// `previewInviteToken` rate limiter.
///
/// Presence of this type in GraphQL context is a boot-time invariant —
/// `build_app_runtime` fails closed if it is absent (§4.7).
#[derive(Debug, Clone, Default)]
pub struct PreviewInviteRateLimiterToken;

/// Zero-sized token type used as a GraphQL context data key for the
/// open-mode signup rate limiter.
///
/// Required when `RIM_OPEN_REGISTRATION=true`; `build_app_runtime` fails
/// closed if it is absent in open mode (§6.3).
#[derive(Debug, Clone, Default)]
pub struct SignupRateLimiterToken;
