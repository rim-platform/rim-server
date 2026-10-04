use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::workspace_state::dto::WorkspaceStateDto;
use crate::workspace_state::request::{
    DeleteWorkspaceStateCmd, GetWorkspaceStateQuery, IncrementWorkspaceStateCmd,
    ListWorkspaceStateQuery, SetWorkspaceStateCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceStateService: Send + Sync {
    async fn get(&self, query: GetWorkspaceStateQuery) -> ServiceResult<Option<WorkspaceStateDto>>;

    async fn list(&self, query: ListWorkspaceStateQuery) -> ServiceResult<Vec<WorkspaceStateDto>>;

    async fn set(&self, cmd: SetWorkspaceStateCmd) -> ServiceResult<WorkspaceStateDto>;

    async fn delete(&self, cmd: DeleteWorkspaceStateCmd) -> ServiceResult<bool>;

    async fn increment(&self, cmd: IncrementWorkspaceStateCmd) -> ServiceResult<WorkspaceStateDto>;
}
