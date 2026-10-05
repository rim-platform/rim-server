use async_graphql::{Enum, InputObject, SimpleObject};

// ── Workspace State ──────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkspaceStateScopeGql {
    Server,
    User,
}

impl From<rim_services::workspace_state::dto::WorkspaceStateScopeDto> for WorkspaceStateScopeGql {
    fn from(value: rim_services::workspace_state::dto::WorkspaceStateScopeDto) -> Self {
        match value {
            rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server => Self::Server,
            rim_services::workspace_state::dto::WorkspaceStateScopeDto::User => Self::User,
        }
    }
}

impl From<WorkspaceStateScopeGql> for rim_services::workspace_state::dto::WorkspaceStateScopeDto {
    fn from(value: WorkspaceStateScopeGql) -> Self {
        match value {
            WorkspaceStateScopeGql::Server => Self::Server,
            WorkspaceStateScopeGql::User => Self::User,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceStateEntryGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub key: String,
    pub value: async_graphql::Json<serde_json::Value>,
    pub scope: WorkspaceStateScopeGql,
    pub user_id: Option<async_graphql::ID>,
    pub version: i64,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::workspace_state::dto::WorkspaceStateDto> for WorkspaceStateEntryGql {
    fn from(value: rim_services::workspace_state::dto::WorkspaceStateDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            key: value.key,
            value: async_graphql::Json(value.value),
            scope: value.scope.into(),
            user_id: value.user_id.map(|id| async_graphql::ID(id.to_string())),
            version: value.version,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct SetWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub value: async_graphql::Json<serde_json::Value>,
    pub scope: WorkspaceStateScopeGql,
    pub ttl_seconds: Option<i64>,
}

#[derive(InputObject, Clone, Debug)]
pub struct DeleteWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub scope: WorkspaceStateScopeGql,
}

#[derive(InputObject, Clone, Debug)]
pub struct IncrementWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub scope: WorkspaceStateScopeGql,
    pub delta: i64,
}
