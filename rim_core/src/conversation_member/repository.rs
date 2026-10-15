use crate::conversation::model::ConversationId;
use crate::conversation_member::model::{ConversationMember, Membership};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

/// Conversation membership. A member is active or pending; `is_member` and the `list_member_ids`
/// methods see active members only, so reads, sends and notifications skip pending ones.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationMemberRepository: Send + Sync {
    /// Adds `user_id` as an active member.
    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember>;

    /// Adds `user_id` as a pending member.
    async fn add_pending_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember>;

    /// Removes `user_id`, active or pending. `NotFound` when not a member.
    async fn remove_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<()>;

    /// Whether `user_id` is an active member.
    async fn is_member(&self, conversation_id: ConversationId, user_id: UserId)
    -> RepoResult<bool>;

    /// `user_id`'s membership, `None` when not a member.
    async fn membership(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<Option<Membership>>;

    /// The active members.
    async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>>;

    /// The active members of each conversation; conversations without any are absent.
    async fn list_member_ids_for_conversations(
        &self,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>>;

    /// The pending members.
    async fn list_pending_member_ids(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Vec<UserId>>;

    /// The pending members of each conversation; conversations without any are absent.
    async fn list_pending_member_ids_for_conversations(
        &self,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>>;

    /// The conversations where `user_id` is a pending member.
    async fn list_pending_conversation_ids(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>>;

    /// Deletes the pending memberships that started before `cutoff`; returns how many.
    async fn delete_pending_before(&self, cutoff: DateTime<Utc>) -> RepoResult<u64>;

    /// Makes a pending member active, joining now. `false` when `user_id` is not a pending
    /// member.
    async fn activate_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<bool>;
}
