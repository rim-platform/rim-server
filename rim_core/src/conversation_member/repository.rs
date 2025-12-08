use async_trait::async_trait;
use crate::conversation::model::ConversationId;
use crate::conversation_member::model::{ConversationMember, ConversationMemberId};
use crate::repository::repository::Repository;
use crate::user::model::UserId;

#[async_trait]
pub trait ConversationMemberRepository<Model = ConversationMember, Id = ConversationMemberId>: Repository
{
    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> anyhow::Result<Self::Model>;

    async fn remove_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> anyhow::Result<()>;

    async fn get_members_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
