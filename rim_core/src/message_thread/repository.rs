use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_thread::model::{MessageThread, MessageThreadId};
use crate::repository::error::RepoResult;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageThreadRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;

    async fn get_by_ids(&self, ids: &[MessageThreadId]) -> RepoResult<Vec<MessageThread>>;

    async fn get_by_root_message(&self, message_id: MessageId)
    -> RepoResult<Option<MessageThread>>;

    async fn get_thread_ids_by_root_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, MessageThreadId>>;

    async fn create_thread_for_root(
        &self,
        conversation_id: ConversationId,
        message_id: MessageId,
    ) -> RepoResult<MessageThread>;

    async fn touch_last_message_at(
        &self,
        thread_id: MessageThreadId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()>;

    /// The number of replies in each of `thread_ids`, deleted ones not counted. A thread without
    /// a live reply is absent from the map.
    async fn count_live_replies(
        &self,
        thread_ids: &[MessageThreadId],
    ) -> RepoResult<HashMap<MessageThreadId, i64>>;
}
