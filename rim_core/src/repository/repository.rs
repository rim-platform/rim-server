use async_trait::async_trait;

#[async_trait]
pub trait Repository: Send + Sync {
    type Model: Send + Sync;
    type Id: Send + Sync;

    async fn get_by_id(&self, id: Self::Id) -> anyhow::Result<Option<Self::Model>>;
    async fn get_all(&self) -> anyhow::Result<Vec<Self::Model>>;
    async fn delete_by_id(&self, id: Self::Id) -> anyhow::Result<()>;
}
