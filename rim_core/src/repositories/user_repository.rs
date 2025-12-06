use crate::repositories::repository::Repository;
use entity::user;
use async_trait::async_trait;

#[async_trait]
pub trait UserRepository<M, Id, T>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
    T: Send + Sync,
{

    async fn create_user(&self, user_data: T) -> anyhow::Result<M>;
    async fn find_by_username(&self, username: &str) -> anyhow::Result<Option<M>>;
    async fn delete_by_username(&self, username: &str) -> anyhow::Result<()>;
}
