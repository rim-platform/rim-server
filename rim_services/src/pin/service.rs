use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::message::model::MessageId;

use crate::ServiceResult;
use crate::message::dto::MessageDto;
use crate::pin::command::{ListPinnedMessagesQuery, PinMessageCmd, UnpinMessageCmd};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PinService: Send + Sync {
    async fn pin_message(&self, cmd: PinMessageCmd) -> ServiceResult<()>;

    async fn unpin_message(&self, cmd: UnpinMessageCmd) -> ServiceResult<()>;

    async fn list_pinned_messages(
        &self,
        query: ListPinnedMessagesQuery,
    ) -> ServiceResult<Vec<MessageDto>>;

    async fn is_pinned_batch(&self, message_ids: &[MessageId]) -> ServiceResult<Vec<MessageId>>;
}
