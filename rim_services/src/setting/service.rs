use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::setting::dto::SettingDto;
use crate::{Actor, ServiceResult};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SettingService: Send + Sync {
    async fn list_settings(&self, actor: Actor) -> ServiceResult<Vec<SettingDto>>;
}
