use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::server::command::{
    CreateServerCmd, DeleteWorkspaceCmd, GetServerQuery, ListMyServersQuery,
    TransferWorkspaceOwnershipCmd,
};
use crate::server::dto::ServerDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerService: Send + Sync {
    async fn create_server(&self, cmd: CreateServerCmd) -> ServiceResult<ServerDto>;
    async fn get_server(&self, query: GetServerQuery) -> ServiceResult<ServerDto>;
    async fn list_my_servers(&self, query: ListMyServersQuery) -> ServiceResult<Vec<ServerDto>>;

    /// Make another member the workspace's owner; the caller becomes a moderator. A workspace
    /// has one owner.
    ///
    /// # Errors
    /// `PermissionDenied` unless the caller is the owner; `NotFound` when the target is not a
    /// member; `Validation` for the direct space, a guest or the caller themself.
    async fn transfer_ownership(
        &self,
        cmd: TransferWorkspaceOwnershipCmd,
    ) -> ServiceResult<ServerDto>;

    /// Delete the workspace with everything in it; every member is told they left it.
    ///
    /// # Errors
    /// `PermissionDenied` unless the caller is the owner; `Validation` for the direct space or
    /// when `confirm_name` is not the workspace's name.
    async fn delete_workspace(&self, cmd: DeleteWorkspaceCmd) -> ServiceResult<bool>;
}
