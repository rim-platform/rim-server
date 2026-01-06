use crate::conversation::model::{Conversation, ConversationId, ConversationType};
use crate::repository::repository::Repository;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use async_trait::async_trait;

#[async_trait]
pub trait ConversationRepository: Repository<Model = Conversation, Id = ConversationId> {
    async fn create_conversation(
        &self,
        conversation_type: ConversationType,
        server_id: ServerId,
    ) -> anyhow::Result<Self::Model>;

    async fn get_conversations_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
