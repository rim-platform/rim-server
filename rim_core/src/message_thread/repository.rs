use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_thread::model::{MessageThread, MessageThreadId};
use crate::repository::repository::Repository;
use async_trait::async_trait;

#[async_trait]
pub trait MessageThreadRepository: Repository<Model = MessageThread, Id = MessageThreadId> {
    async fn create_thread(
        &self,
        conversation_id: ConversationId,
        first_message_id: Option<MessageId>,
    ) -> anyhow::Result<Self::Model>;

    async fn set_first_message(
        &self,
        thread_id: MessageThreadId,
        message_id: MessageId,
    ) -> anyhow::Result<()>;

    async fn get_threads_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
