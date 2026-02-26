use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::server_member::command::{
    AddServerMemberCmd, GetServerMemberQuery, LeaveServerCmd, ListServerMembersQuery,
    RemoveServerMemberCmd,
};
use crate::server_member::dto::ServerMemberDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerMemberService: Send + Sync {
    async fn add_member(&self, cmd: AddServerMemberCmd) -> ServiceResult<ServerMemberDto>;

    async fn remove_member(&self, cmd: RemoveServerMemberCmd) -> ServiceResult<()>;

    async fn leave(&self, cmd: LeaveServerCmd) -> ServiceResult<()>;

    async fn get_member(&self, query: GetServerMemberQuery) -> ServiceResult<ServerMemberDto>;

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>>;
}
