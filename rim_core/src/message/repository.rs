use crate::conversation::model::ConversationId;
use crate::message::model::{Message, MessageId, NewMessage};
use crate::message_thread::model::MessageThreadId;
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir, SortOrder};
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCursor {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: MessageId,
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait MessageRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;

    async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>>;

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;

    #[allow(clippy::too_many_arguments)]
    async fn list_messages(
        &self,
        conversation_id: ConversationId,
        thread_id: Option<MessageThreadId>,
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

    async fn set_has_attachments(
        &self,
        message_id: MessageId,
        has_attachments: bool,
    ) -> RepoResult<Message>;

    async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
}
