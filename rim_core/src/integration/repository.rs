use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::integration::model::Integration;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IntegrationRepository: Send + Sync {
    async fn list_integrations(&self) -> RepoResult<Vec<Integration>>;
    async fn get_integration(&self, id: &str) -> RepoResult<Option<Integration>>;
}
