use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::event::dto::EventDto;
use crate::{Actor, ServiceResult};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EventService: Send + Sync {
    async fn list_events(&self, actor: Actor) -> ServiceResult<Vec<EventDto>>;
}
