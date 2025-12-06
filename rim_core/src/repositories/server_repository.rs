use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait ServerRepository<M, Id>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn create_server(&self, name: &str) -> anyhow::Result<M>;

    async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<M>>;

    async fn get_servers_for_user(&self, user_id: Id) -> anyhow::Result<Vec<M>>;
}
