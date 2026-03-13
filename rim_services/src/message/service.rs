use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, GetMessageReferenceQuery, ListMessagesQuery,
    ListThreadMessagesQuery, SearchMessagesQuery, SendMessageCmd, SendThreadMessageCmd,
};
use crate::message::dto::{MessageDto, MessagePageCursor, MessageReferenceDto};
use rim_core::repository::pagination::Page;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageService: Send + Sync {
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto>;

    async fn send_thread_message(&self, cmd: SendThreadMessageCmd) -> ServiceResult<MessageDto>;

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto>;

    async fn get_message_reference(
        &self,
        query: GetMessageReferenceQuery,
    ) -> ServiceResult<MessageReferenceDto>;

    async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> ServiceResult<Page<MessageDto, MessagePageCursor>>;

    async fn list_thread_messages(
        &self,
        query: ListThreadMessagesQuery,
    ) -> ServiceResult<Page<MessageDto, MessagePageCursor>>;

    async fn search_messages(&self, query: SearchMessagesQuery) -> ServiceResult<Vec<MessageDto>>;

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto>;

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()>;
}
