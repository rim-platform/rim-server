use crate::model_id;
use chrono::{DateTime, Utc};

use crate::server::model::ServerId;
use crate::server_member::model::ServerMemberRole;
use crate::user::model::UserId;

model_id!(SessionId);
model_id!(InviteId);
model_id!(BootstrapTokenId);
model_id!(InstanceInviteId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    /// The device a key login or enrollment bound the session to.
    pub device_key_id: Option<crate::key_identity::model::DeviceKeyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    pub user_id: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: InviteId,
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
    /// The role a user joining through this invite gets: `Member` (default) or `Guest`.
    pub role: ServerMemberRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInvite {
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub token_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub role: ServerMemberRole,
}

/// A one-time bootstrap token issued by the CLI.
///
/// The raw token is never stored; only the SHA-256 hash is persisted.
/// Authenticity is established by hash lookup (§3.3 of the registration spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapToken {
    pub id: BootstrapTokenId,
    /// Informational email baked in at issuance. Not a hard constraint on signup.
    pub email: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub consumed_by_user_id: Option<UserId>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Fields required to persist a new bootstrap token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBootstrapToken {
    pub email: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// An instance-level invite that lets a stranger create an account.
///
/// Unlike workspace invites, instance invites are not bound to any workspace.
/// The consumer lands in "no workspaces yet" state after signup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInvite {
    pub id: InstanceInviteId,
    /// The user who issued it; `None` for one the operator issued from the command line.
    pub created_by_user_id: Option<UserId>,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Fields required to persist a new instance invite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInstanceInvite {
    /// The issuing user; `None` for the operator.
    pub created_by_user_id: Option<UserId>,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub max_uses: i32,
}
