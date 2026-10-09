use crate::Actor;
use crate::workspace_state::dto::WorkspaceStateScopeDto;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct GetWorkspaceStateQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub key: String,
    pub scope: WorkspaceStateScopeDto,
}

#[derive(Debug, Clone)]
pub struct ListWorkspaceStateQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub prefix: String,
    pub scope: WorkspaceStateScopeDto,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SetWorkspaceStateCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub key: String,
    pub value: serde_json::Value,
    pub scope: WorkspaceStateScopeDto,
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct DeleteWorkspaceStateCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub key: String,
    pub scope: WorkspaceStateScopeDto,
}

#[derive(Debug, Clone)]
pub struct IncrementWorkspaceStateCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub key: String,
    pub scope: WorkspaceStateScopeDto,
    pub delta: i64,
}
