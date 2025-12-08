use crate::repository::repository::Repository;
use async_trait::async_trait;
use crate::user::model::{NewUser, User, UserId};

#[async_trait]
pub trait UserRepository<Model = User, Id = UserId>: Repository
{
    async fn create_user(&self, user_data: NewUser) -> anyhow::Result<Self::Model>;
    async fn find_by_username(&self, username: &str) -> anyhow::Result<Option<Self::Model>>;
    async fn delete_by_username(&self, username: &str) -> anyhow::Result<()>;
}
