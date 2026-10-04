use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::event::model::Event;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EventRepository: Send + Sync {
    async fn list_events(&self) -> RepoResult<Vec<Event>>;
    async fn get_event(&self, id: &str) -> RepoResult<Option<Event>>;
}
