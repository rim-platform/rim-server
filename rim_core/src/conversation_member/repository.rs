use crate::conversation::model::ConversationId;
use crate::conversation_member::model::{ConversationMember, ConversationMemberId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;
use async_trait::async_trait;

#[async_trait]
pub trait ConversationMemberRepository: Send + Sync {
    async fn get_by_id(&self, id: ConversationMemberId) -> RepoResult<Option<ConversationMember>>;

    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember>;

    async fn remove_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<()>;

    async fn get_members_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Vec<ConversationMember>>;

    async fn is_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId
    ) -> RepoResult<bool>;

    async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>>;
}
