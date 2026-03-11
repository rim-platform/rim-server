use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::integration::dto::IntegrationDto;
use crate::{Actor, ServiceResult};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IntegrationService: Send + Sync {
    async fn list_integrations(&self, actor: Actor) -> ServiceResult<Vec<IntegrationDto>>;
}
