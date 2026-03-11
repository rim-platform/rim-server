use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use crate::capability::model::Capability;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait CapabilityRepository: Send + Sync {
    async fn list_capabilities(&self) -> RepoResult<Vec<Capability>>;
    async fn get_capability(&self, id: &str) -> RepoResult<Option<Capability>>;
}
