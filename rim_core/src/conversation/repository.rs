use async_trait::async_trait;
use crate::conversation::model::{Conversation, ConversationId, ConversationType};
use crate::repository::repository::Repository;
use crate::user::model::UserId;

#[async_trait]
pub trait ConversationRepository<Model = Conversation, Id = ConversationId>: Repository
{
    async fn create_conversation(
        &self,
        conversation_type: ConversationType,
    ) -> anyhow::Result<Self::Model>;

    async fn get_conversations_for_user(
        &self,
        user_id: UserId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
