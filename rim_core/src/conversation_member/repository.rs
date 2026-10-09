use crate::conversation::model::ConversationId;
use crate::conversation_member::model::{ConversationMember, ConversationMemberId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
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

    async fn is_member(&self, conversation_id: ConversationId, user_id: UserId)
    -> RepoResult<bool>;

    async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>>;

    async fn list_member_ids_for_conversations(
        &self,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>>;
}
