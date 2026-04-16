use chrono::{DateTime, Utc};

use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(WorkspaceStateId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceStateScope {
    Server,
    User,
}

impl WorkspaceStateScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceState {
    pub id: WorkspaceStateId,
    pub server_id: ServerId,
    pub key: String,
    pub value: serde_json::Value,
    pub scope: WorkspaceStateScope,
    pub user_id: Option<UserId>,
    pub created_by: UserId,
    pub updated_by: UserId,
    pub version: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewWorkspaceState {
    pub server_id: ServerId,
    pub key: String,
    pub value: serde_json::Value,
    pub scope: WorkspaceStateScope,
    pub user_id: Option<UserId>,
    pub created_by: UserId,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWorkspaceState {
    pub value: serde_json::Value,
    pub updated_by: UserId,
    pub expires_at: Option<DateTime<Utc>>,
    pub expected_version: Option<i64>,
}
