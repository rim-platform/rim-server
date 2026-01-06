use crate::repository::repository::Repository;
use crate::user::model::{NewUser, User, UserId};
use async_trait::async_trait;

#[async_trait]
pub trait UserRepository: Repository<Model = User, Id = UserId> {
    async fn create_user(&self, user_data: NewUser) -> anyhow::Result<Self::Model>;
    async fn find_by_username(&self, username: &str) -> anyhow::Result<Option<Self::Model>>;
    async fn delete_by_username(&self, username: &str) -> anyhow::Result<()>;
}
