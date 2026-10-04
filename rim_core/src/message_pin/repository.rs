use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_pin::model::MessagePin;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessagePinRepository: Send + Sync {
    async fn pin(
        &self,
        message_id: MessageId,
        conversation_id: ConversationId,
        pinned_by: UserId,
    ) -> RepoResult<MessagePin>;

    async fn unpin(&self, message_id: MessageId) -> RepoResult<()>;

    async fn list_pinned(&self, conversation_id: ConversationId) -> RepoResult<Vec<MessagePin>>;

    async fn is_pinned(&self, message_id: MessageId) -> RepoResult<bool>;

    async fn is_pinned_batch(&self, message_ids: &[MessageId]) -> RepoResult<Vec<MessageId>>;

    async fn count_pins(&self, conversation_id: ConversationId) -> RepoResult<u64>;
}
