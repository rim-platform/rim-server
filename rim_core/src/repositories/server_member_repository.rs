use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait ServerMemberRepository<M, Id, RT>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
    RT: Send + Sync,
{
    async fn add_member(
        &self,
        server_id: Id,
        user_id: Id,
        role: RT,
    ) -> anyhow::Result<M>;

    async fn change_member_role(
        &self,
        server_id: Id,
        user_id: Id,
        role: RT,
    ) -> anyhow::Result<()>;

    async fn remove_member(&self, server_id: Id, user_id: Id) -> anyhow::Result<()>;

    async fn get_members_for_server(&self, server_id: Id) -> anyhow::Result<Vec<M>>;
}
