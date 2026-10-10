use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message_thread::command::{GetThreadByRootMessageQuery, GetThreadQuery};
use crate::message_thread::dto::ThreadDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ThreadService: Send + Sync {
    async fn get_thread_by_root_message(
        &self,
        query: GetThreadByRootMessageQuery,
    ) -> ServiceResult<Option<ThreadDto>>;

    async fn get_thread(&self, query: GetThreadQuery) -> ServiceResult<ThreadDto>;
}
