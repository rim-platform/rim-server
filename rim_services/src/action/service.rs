use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::action::dto::ActionDto;
use crate::{Actor, ServiceResult};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ActionService: Send + Sync {
    async fn list_actions(&self, actor: Actor) -> ServiceResult<Vec<ActionDto>>;
}
