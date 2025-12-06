use async_trait::async_trait;

#[async_trait]
pub trait Repository<M, Id>: Send + Sync
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn get_by_id(&self, id: Id) -> anyhow::Result<Option<M>>;
    async fn get_all(&self) -> anyhow::Result<Vec<M>>;
    async fn delete_by_id(&self, id: Id) -> anyhow::Result<()>;
}
