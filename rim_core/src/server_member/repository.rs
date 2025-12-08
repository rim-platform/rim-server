use async_trait::async_trait;
use crate::repository::repository::Repository;
use crate::server::model::ServerId;
use crate::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use crate::user::model::UserId;

#[async_trait]
pub trait ServerMemberRepository<Model = ServerMember, Id = ServerMemberId>: Repository
{
    async fn add_member(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> anyhow::Result<Self::Model>;

    async fn change_member_role(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> anyhow::Result<()>;

    async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> anyhow::Result<()>;

    async fn get_members_for_server(&self, server_id: ServerId) -> anyhow::Result<Vec<Self::Model>>;
}
