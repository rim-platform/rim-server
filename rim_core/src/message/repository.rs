use crate::message::model::{Message, MessageId, NewMessage};
use crate::message_thread::model::MessageThreadId;
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir, SortOrder};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCursor {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: MessageId,
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait MessageRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;

    async fn list_messages(
        &self,
        thread_id: MessageThreadId,
        limit: u32,
        dir: PageDir,
        cursor: Option<MessageCursor>,
        include_deleted: bool,
        order: SortOrder,
    ) -> RepoResult<Page<Message, MessageCursor>>;

    async fn update_message_text(
        &self,
        message_id: MessageId,
        new_text: String,
    ) -> RepoResult<Message>;

    async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
}
