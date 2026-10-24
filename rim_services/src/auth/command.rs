use chrono::{DateTime, Utc};

use rim_core::server::model::ServerId;

use crate::Actor;

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
    /// Role granted on join: `Member` or `Guest`. Anything else is a
    /// [`crate::ServiceError::Validation`] error.
    pub role: rim_core::server_member::model::ServerMemberRole,
}

/// Revoke a workspace invite of a workspace the actor owns.
#[derive(Debug, Clone)]
pub struct RevokeInviteCmd {
    pub actor: Actor,
    pub invite_id: rim_core::auth::model::InviteId,
}

/// The invites of a workspace the actor owns, newest first.
#[derive(Debug, Clone)]
pub struct ListInvitesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub dir: rim_core::repository::pagination::PageDir,
    pub cursor: Option<crate::auth::dto::InvitePageCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct JoinServerByInviteCmd {
    pub actor: Actor,
    pub invite_token: String,
}

/// The registration token a key signup presents in invite-only mode, by kind. Each takes one
/// use inside the signup's transaction: an instance invite creates an account with no workspace
/// of its own; a workspace invite also joins its server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignupToken {
    InstanceInvite(String),
    WorkspaceInvite(String),
}

/// Query to preview a registration token (instance or workspace invite).
#[derive(Debug, Clone)]
pub struct PreviewInviteTokenQuery {
    pub token: String,
    pub remote_ip: String,
}
