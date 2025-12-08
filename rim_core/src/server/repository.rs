use async_trait::async_trait;
use crate::repository::repository::Repository;
use crate::server::model::{Server, ServerId};
use crate::user::model::UserId;

#[async_trait]
pub trait ServerRepository<Model = Server, Id = ServerId>: Repository
{
    async fn create_server(&self, name: &str) -> anyhow::Result<Self::Model>;

    async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<Self::Model>>;

    async fn get_servers_for_user(&self, user_id: UserId) -> anyhow::Result<Vec<Self::Model>>;
}
