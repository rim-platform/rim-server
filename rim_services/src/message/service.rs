use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessagesQuery, SendMessageCmd,
};
use crate::message::dto::{MessageDto, MessagePageCursor};
use rim_core::repository::pagination::Page;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageService: Send + Sync {
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto>;

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto>;

    async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> ServiceResult<Page<MessageDto, MessagePageCursor>>;

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto>;

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()>;
}
