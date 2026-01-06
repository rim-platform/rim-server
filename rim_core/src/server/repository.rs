use crate::repository::repository::Repository;
use crate::server::model::{Server, ServerId};
use crate::user::model::UserId;
use async_trait::async_trait;

#[async_trait]
pub trait ServerRepository: Repository<Model = Server, Id = ServerId> {
    async fn create_server(&self, name: &str) -> anyhow::Result<Self::Model>;

    async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<Self::Model>>;

    async fn get_servers_for_user(&self, user_id: UserId) -> anyhow::Result<Vec<Self::Model>>;
}
