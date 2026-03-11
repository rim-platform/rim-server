use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use crate::action::model::Action;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait ActionRepository: Send + Sync {
    async fn list_actions(&self) -> RepoResult<Vec<Action>>;
    async fn get_action(&self, id: &str) -> RepoResult<Option<Action>>;
}
