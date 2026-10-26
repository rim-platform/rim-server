use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::message_thread::model::MessageThreadId;

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

    /// The number of replies in each of `thread_ids`, deleted ones not counted; a thread without
    /// a live reply is absent from the map. Checks no access: the caller has already read the
    /// root messages.
    async fn count_replies(
        &self,
        thread_ids: &[MessageThreadId],
    ) -> ServiceResult<HashMap<MessageThreadId, i64>>;
}
