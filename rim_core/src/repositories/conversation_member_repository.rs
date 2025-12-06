use async_trait::async_trait;
use crate::repositories::repository::Repository;

#[async_trait]
pub trait ConversationMemberRepository<M, Id>:
Repository<M, Id>
where
    M: Send + Sync,
    Id: Send + Sync,
{
    async fn add_member(
        &self,
        conversation_id: Id,
        user_id: Id,
    ) -> anyhow::Result<M>;

    async fn remove_member(
        &self,
        conversation_id: Id,
        user_id: Id,
    ) -> anyhow::Result<()>;

    async fn get_members_for_conversation(
        &self,
        conversation_id: Id,
    ) -> anyhow::Result<Vec<M>>;
}
