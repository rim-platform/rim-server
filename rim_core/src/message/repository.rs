use crate::message::model::{Message, MessageId, NewMessage};
use crate::message_thread::model::MessageThreadId;
use crate::repository::repository::Repository;
use crate::user::model::UserId;
use async_trait::async_trait;

#[async_trait]
pub trait MessageRepository: Repository<Model = Message, Id = MessageId> {
    async fn create_message(
        &self,
        new_message: &NewMessage,
    ) -> anyhow::Result<Self::Model>;

    async fn delete_message(&self, message_id: Self::Id) -> anyhow::Result<()>;

    async fn get_messages_for_thread(
        &self,
        thread_id: MessageThreadId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
