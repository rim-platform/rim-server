use async_trait::async_trait;

use crate::ServiceResult;
use crate::message_thread::command::{GetThreadQuery, ListThreadsQuery};
use crate::message_thread::dto::{ThreadDto, ThreadPageCursor};
use rim_core::repository::pagination::Page;

#[async_trait]
pub trait ThreadService: Send + Sync {
    async fn get_thread(&self, query: GetThreadQuery) -> ServiceResult<ThreadDto>;

    async fn list_threads(
        &self,
        query: ListThreadsQuery,
    ) -> ServiceResult<Page<ThreadDto, ThreadPageCursor>>;
}
