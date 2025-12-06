use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait ConversationRepository<M, Id, CT>: Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
    CT: Send + Sync,
{
    /// Create a conversation of given type (direct / group / channel).
    async fn create_conversation(
        &self,
        conversation_type: CT,
    ) -> anyhow::Result<M>;

    /// Get all conversations where the user is a member.
    async fn get_conversations_for_user(
        &self,
        user_id: Id,
    ) -> anyhow::Result<Vec<M>>;
}