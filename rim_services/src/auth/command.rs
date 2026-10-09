use chrono::{DateTime, Utc};

use rim_core::auth::model::InstanceInviteId;
use rim_core::server::model::ServerId;

use crate::Actor;

#[derive(Debug, Clone)]
pub struct LoginCmd {
    pub username: String,
    pub password: String,
    /// Client IP for per-IP login rate limiting (`"unknown"` when unavailable).
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct LogoutCmd {
    pub actor: Actor,
    pub session_id: rim_core::auth::model::SessionId,
}

/// Sign out every session of the actor's account except `session_id`, the calling one.
#[derive(Debug, Clone)]
pub struct LogoutOtherSessionsCmd {
    pub actor: Actor,
    pub session_id: rim_core::auth::model::SessionId,
}

#[derive(Debug, Clone)]
pub struct CreateInviteCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    /// Optional recipient to email the invite link to. Validated server-side (basic format
    /// check) before the invite is persisted; malformed addresses return
    /// [`crate::ServiceError::Validation`]. Send failure is best-effort — the invite is still
    /// created and its raw token still returned even if the email doesn't go out.
    pub recipient_email: Option<String>,
    /// Role granted on join (Track 1.2): `Member` or `Guest`. Anything else is a
    /// [`crate::ServiceError::Validation`] error.
    pub role: rim_core::server_member::model::ServerMemberRole,
}

#[derive(Debug, Clone)]
pub struct AcceptInviteCmd {
    pub invite_token: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct JoinServerByInviteCmd {
    pub actor: Actor,
    pub invite_token: String,
}

#[derive(Debug, Clone)]
pub struct ListInvitesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct RevokeInviteCmd {
    pub actor: Actor,
    pub invite_id: rim_core::auth::model::InviteId,
}

/// The registration token a signup presents in invite-only mode, by kind. Each kind is
/// checked and consumed the way its password signup does it: a bootstrap token or an instance
/// invite creates an account with no workspace; a workspace invite also joins its server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignupToken {
    Bootstrap(String),
    InstanceInvite(String),
    WorkspaceInvite(String),
}

/// Command for signing up with a bootstrap token.
#[derive(Debug, Clone)]
pub struct SignupWithBootstrapCmd {
    pub bootstrap_token: String,
    pub username: String,
    pub password: String,
    /// Required. Pre-normalised (trimmed + lowercased) at the resolver boundary;
    /// the service also validates before insert as defence in depth.
    pub email: String,
    /// Optional display name (raw input). The service normalises and falls back
    /// to `username` when absent or blank after trim.
    pub display_name: Option<String>,
}

/// Command for signing up with an instance invite token.
#[derive(Debug, Clone)]
pub struct SignupWithInstanceInviteCmd {
    pub instance_invite_token: String,
    pub username: String,
    pub password: String,
    /// Required. Pre-normalised (trimmed + lowercased) at the resolver boundary;
    /// the service also validates before insert as defence in depth.
    pub email: String,
    /// Optional display name (raw input). The service normalises and falls back
    /// to `username` when absent or blank after trim.
    pub display_name: Option<String>,
}

/// Command for open-mode signup (no token required).
#[derive(Debug, Clone)]
pub struct SignupOpenCmd {
    pub username: String,
    pub password: String,
    /// Caller's remote IP for rate-limit enforcement.
    ///
    /// The service checks the signup rate limiter against this IP before
    /// creating the user, returning `RateLimited` if the limit is exceeded.
    pub remote_ip: String,
    /// Required. Pre-normalised (trimmed + lowercased) at the resolver boundary;
    /// the service also validates before insert as defence in depth.
    pub email: String,
    /// Optional display name (raw input). The service normalises and falls back
    /// to `username` when absent or blank after trim.
    pub display_name: Option<String>,
}

/// Command for an admin to create a new instance invite.
#[derive(Debug, Clone)]
pub struct CreateInstanceInviteCmd {
    pub actor: Actor,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
    /// Optional recipient to email the invite link to. Same validation/best-effort-send
    /// semantics as [`CreateInviteCmd::recipient_email`].
    pub recipient_email: Option<String>,
}

/// Command for an admin to revoke an existing instance invite.
#[derive(Debug, Clone)]
pub struct RevokeInstanceInviteCmd {
    pub actor: Actor,
    pub invite_id: InstanceInviteId,
}

/// Query to preview a registration token (bootstrap, instance, or workspace).
#[derive(Debug, Clone)]
pub struct PreviewInviteTokenQuery {
    pub token: String,
    pub remote_ip: String,
}
