use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::server_member::command::{GetServerMemberQuery, ListServerMembersQuery};
use crate::server_member::dto::ServerMemberDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerMemberService: Send + Sync {
    async fn get_member(&self, query: GetServerMemberQuery) -> ServiceResult<ServerMemberDto>;

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>>;
}
