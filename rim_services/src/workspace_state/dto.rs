use chrono::{DateTime, Utc};

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_state::model::WorkspaceStateId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkspaceStateScopeDto {
    Server,
    User,
}

impl From<rim_core::workspace_state::model::WorkspaceStateScope> for WorkspaceStateScopeDto {
    fn from(value: rim_core::workspace_state::model::WorkspaceStateScope) -> Self {
        match value {
            rim_core::workspace_state::model::WorkspaceStateScope::Server => Self::Server,
            rim_core::workspace_state::model::WorkspaceStateScope::User => Self::User,
        }
    }
}

impl From<WorkspaceStateScopeDto> for rim_core::workspace_state::model::WorkspaceStateScope {
    fn from(value: WorkspaceStateScopeDto) -> Self {
        match value {
            WorkspaceStateScopeDto::Server => Self::Server,
            WorkspaceStateScopeDto::User => Self::User,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceStateDto {
    pub id: WorkspaceStateId,
    pub server_id: ServerId,
    pub key: String,
    pub value: serde_json::Value,
    pub scope: WorkspaceStateScopeDto,
    pub user_id: Option<UserId>,
    pub version: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<rim_core::workspace_state::model::WorkspaceState> for WorkspaceStateDto {
    fn from(m: rim_core::workspace_state::model::WorkspaceState) -> Self {
        Self {
            id: m.id,
            server_id: m.server_id,
            key: m.key,
            value: m.value,
            scope: m.scope.into(),
            user_id: m.user_id,
            version: m.version,
            expires_at: m.expires_at,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}
