use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_thread::model::{MessageThread, MessageThreadId};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadCursor {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: MessageThreadId,
}

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

    /// Creates an empty thread. For the typical "thread per root message" flow, you create a
    /// thread first, then create the root message with that thread id, then call
    /// `set_first_message`.
    async fn create_thread(&self, conversation_id: ConversationId) -> RepoResult<MessageThread>;

    async fn create_thread_for_root(
        &self,
        conversation_id: ConversationId,
        message_id: MessageId,
    ) -> RepoResult<MessageThread>;

    async fn set_first_message(
        &self,
        thread_id: MessageThreadId,
        message_id: MessageId,
    ) -> RepoResult<()>;

    async fn touch_last_message_at(
        &self,
        thread_id: MessageThreadId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()>;

    /// List threads ordered by activity.
    ///
    /// Recommended ordering for a thread-first UI is `last_message_at DESC, id DESC`.
    async fn list_threads_for_conversation(
        &self,
        conversation_id: ConversationId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ThreadCursor>,
    ) -> RepoResult<Page<MessageThread, ThreadCursor>>;
}
