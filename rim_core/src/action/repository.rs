use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::action::model::Action;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ActionRepository: Send + Sync {
    async fn list_actions(&self) -> RepoResult<Vec<Action>>;
    async fn get_action(&self, id: &str) -> RepoResult<Option<Action>>;
}
