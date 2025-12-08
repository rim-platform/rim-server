use async_trait::async_trait;
use crate::message::model::{Message, MessageId};
use crate::message_thread::model::MessageThreadId;
use crate::repository::repository::Repository;
use crate::user::model::UserId;

#[async_trait]
pub trait MessageRepository<Model = Message, Id = MessageId>: Repository
{
    async fn create_message(
        &self,
        thread_id: MessageThreadId,
        parent_message_id: Option<Self::Id>,
        sent_by: UserId,
        text: &str,
        has_attachments: bool,
    ) -> anyhow::Result<Self::Model>;

    async fn delete_message(&self, message_id: Self::Id) -> anyhow::Result<()>;

    async fn get_messages_for_thread(
        &self,
        thread_id: MessageThreadId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
