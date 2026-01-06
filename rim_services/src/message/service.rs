use async_trait::async_trait;

use crate::ServiceResult;
use crate::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessagesQuery, SendMessageCmd,
};
use crate::message::dto::{MessageDto};

#[async_trait]
pub trait MessageService: Send + Sync {
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto>;

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto>;

    async fn list_messages(&self, query: ListMessagesQuery) -> ServiceResult<Vec<MessageDto>>;

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto>;

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()>;
}
