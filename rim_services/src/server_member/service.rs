use std::collections::HashSet;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::user::model::UserId;

use crate::ServiceResult;
use crate::server_member::command::{
    GetServerMemberQuery, GetServerOwnerQuery, ListServerMembersQuery,
};
use crate::server_member::dto::ServerMemberDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerMemberService: Send + Sync {
    async fn get_member(&self, query: GetServerMemberQuery) -> ServiceResult<ServerMemberDto>;

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>>;

    /// The members the actor may see in the workspace: `None` when every member is visible,
    /// otherwise a guest's own id and everyone who shares a conversation with them.
    /// `PermissionDenied` for a non-member.
    async fn visible_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Option<HashSet<UserId>>>;

    /// The id of the workspace's owner, `None` when it has none. Any member may ask, guests
    /// included. `PermissionDenied` for a non-member.
    async fn get_owner(&self, query: GetServerOwnerQuery) -> ServiceResult<Option<UserId>>;
}
