use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::server_member::model::{MemberStanding, ServerMember, ServerMemberRole};
use crate::user::model::UserId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait ServerMemberRepository: Send + Sync {
    async fn get_member(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<ServerMember>>;

    async fn add_member(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> RepoResult<ServerMember>;

    async fn change_member_role(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> RepoResult<()>;

    async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;

    async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;

    async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;

    /// The member's role plus any timeout still in force (`until > now()`), in one call.
    /// `None` when the user is not a member. Used by the conversation access gates (Track 1.2).
    async fn get_member_standing(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<MemberStanding>>;
}
