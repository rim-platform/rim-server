use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_thread::model::{MessageThread, MessageThreadId};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadCursor {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: MessageThreadId,
}

#[async_trait]
pub trait MessageThreadRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;

    /// Creates an empty thread. For the typical "thread per root message" flow, you create a
    /// thread first, then create the root message with that thread id, then call
    /// `set_first_message`.
    async fn create_thread(&self, conversation_id: ConversationId) -> RepoResult<MessageThread>;

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
