use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::capability::dto::CapabilityDto;
use crate::{Actor, ServiceResult};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CapabilityService: Send + Sync {
    async fn list_capabilities(&self, actor: Actor) -> ServiceResult<Vec<CapabilityDto>>;
}
