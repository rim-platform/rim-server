use crate::repository::error::RepoResult;
use crate::server::model::{Server, ServerId};
use crate::user::model::UserId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait ServerRepository: Send + Sync {
    async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;

    async fn create_server(&self, name: &str) -> RepoResult<Server>;

    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Server>>;

    async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>>;
}
