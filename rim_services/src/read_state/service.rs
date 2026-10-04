use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::read_state::dto::{MarkAllAsReadCmd, MarkAsReadCmd, UnreadCountDto, UnreadCountsQuery};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ReadStateService: Send + Sync {
    async fn mark_as_read(&self, cmd: MarkAsReadCmd) -> ServiceResult<()>;

    async fn mark_all_as_read(&self, cmd: MarkAllAsReadCmd) -> ServiceResult<()>;

    async fn get_unread_counts(
        &self,
        query: UnreadCountsQuery,
    ) -> ServiceResult<Vec<UnreadCountDto>>;
}
