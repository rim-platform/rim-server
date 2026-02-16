use crate::model_id;
use chrono::{DateTime, Utc};

use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(SessionId);
model_id!(InviteId);

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInvite {
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub token_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
}
