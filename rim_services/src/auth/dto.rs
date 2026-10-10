use chrono::{DateTime, Utc};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct AuthSessionDto {
    pub user_id: UserId,
    pub session_id: rim_core::auth::model::SessionId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct InviteDto {
    pub id: rim_core::auth::model::InviteId,
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    pub role: rim_core::server_member::model::ServerMemberRole,
}

#[derive(Debug, Clone)]
pub struct LoginResultDto {
    pub user_id: UserId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
pub struct CreateInviteResultDto {
    pub invite: InviteDto,
    pub invite_token: String,
}

#[derive(Debug, Clone)]
pub struct AcceptInviteResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
pub struct JoinServerByInviteResultDto {
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Default)]
pub struct RequestAuthResultDto {
    pub user_id: Option<UserId>,
    pub session_id: Option<rim_core::auth::model::SessionId>,
    /// The device the session is bound to.
    pub device_key_id: Option<rim_core::key_identity::model::DeviceKeyId>,
}

/// Result of any signup path (bootstrap, instance-invite, open).
///
/// `server_id` is `Some` only when a workspace invite was used;
/// bootstrap and instance-invite signups land in "no workspaces yet" state.
#[derive(Debug, Clone)]
pub struct SignupResultDto {
    pub user_id: UserId,
    pub server_id: Option<ServerId>,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

/// Token type discriminant for the preview query.
///
/// `Bootstrap` and `Instance` both lead to "create your RIM account" copy on
/// the frontend; they are now distinct so the frontend knows which input field
/// to use on `signupWithInvite` (`bootstrapToken` vs `instanceInviteToken`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteTokenType {
    /// Bootstrap token: "create your RIM account" flow via `bootstrapToken` input.
    Bootstrap,
    /// Instance invite: "create your RIM account" flow via `instanceInviteToken` input.
    Instance,
    /// Workspace invite: "join workspace X" copy.
    Workspace,
}

/// Minimal workspace info included in a workspace-invite preview.
#[derive(Debug, Clone)]
pub struct WorkspaceInvitePreviewDto {
    pub server_id: ServerId,
    pub server_name: String,
    pub invited_by_display_name: String,
}

/// Read-only preview of a registration token.
///
/// Does NOT consume the token. See spec §4.7.
#[derive(Debug, Clone)]
pub struct InviteTokenPreviewDto {
    pub token_type: InviteTokenType,
    pub expires_at: Option<DateTime<Utc>>,
    /// Populated only when `token_type == Workspace`.
    pub workspace: Option<WorkspaceInvitePreviewDto>,
}

/// The registration mode in effect for this deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationMode {
    /// Default: signup requires a valid token (bootstrap, instance, or workspace invite).
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
    /// `RIM_REGISTRATION_POW_BITS`: a key-only signup takes a registration token or a
    /// proof-of-work of `bits` leading zero bits (rate-limited as open signups); a password
    /// signup takes a token, as in invite-only mode.
    Pow { bits: u32 },
}

/// Zero-sized marker type carried through [`RegistrationContext`] as a GraphQL
/// context data key for the `previewInviteToken` rate limiter.
///
/// The actual limiter is constructed inside `DefaultAuthService::new()` and is
/// always present — "missing limiter" is a state the type system makes
/// unrepresentable (spec §4.7 R2). This type exists for future-extensibility:
/// a caller can swap in a custom limiter implementation by replacing this marker
/// without changing the `RegistrationContext` shape.
///
/// [`RegistrationContext`]: rim_api::RegistrationContext
#[derive(Debug, Clone, Default)]
pub struct PreviewInviteRateLimiterToken;

/// Zero-sized marker type carried through [`RegistrationContext`] as a GraphQL
/// context data key for the open-mode signup rate limiter.
///
/// The actual limiter is constructed inside `DefaultAuthService::new()` and is
/// always present (spec §6.3 R2). This type exists for future-extensibility:
/// a caller can swap in a custom limiter implementation without changing the
/// `RegistrationContext` shape.
///
/// [`RegistrationContext`]: rim_api::RegistrationContext
#[derive(Debug, Clone, Default)]
pub struct SignupRateLimiterToken;
