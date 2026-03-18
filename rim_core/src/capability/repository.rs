use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::capability::model::Capability;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CapabilityRepository: Send + Sync {
    async fn list_capabilities(&self) -> RepoResult<Vec<Capability>>;
    async fn get_capability(&self, id: &str) -> RepoResult<Option<Capability>>;
}
