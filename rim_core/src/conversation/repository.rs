use crate::conversation::model::{Conversation, ConversationId, ConversationType, NewConversation};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;
use crate::user::model::UserId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationActivityCursor {
    pub last_activity_at: DateTime<Utc>,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationCreatedCursor {
    pub created_at: DateTime<Utc>,
    pub conversation_id: ConversationId,
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationRepository: Send + Sync {
    async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;

    async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation>;

    async fn create_conversation_with_members(
        &self,
        conversation_type: ConversationType,
        server_id: ServerId,
        title: Option<String>,
        member_ids: &[UserId],
    ) -> RepoResult<Conversation>;

    async fn find_direct_between_users(
        &self,
        server_id: ServerId,
        user_a: UserId,
        user_b: UserId,
    ) -> RepoResult<Option<Conversation>>;

    /// List DM + Group conversations for a user ("inbox" semantics).
    ///
    /// Ordering: `last_activity_at DESC, id DESC`.
    async fn list_inbox_conversations_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationActivityCursor>,
    ) -> RepoResult<Page<Conversation, ConversationActivityCursor>>;

    /// List channel conversations for a user in a stable order.
    ///
    /// v0 ordering: `created_at ASC, id ASC`.
    async fn list_channels_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationCreatedCursor>,
    ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;

    /// Check whether a channel with the given title already exists on the server.
    async fn exists_channel_by_title(&self, server_id: ServerId, title: &str) -> RepoResult<bool>;

    /// Update the conversation activity timestamp.
    /// Intended to be called when a new message is persisted.
    async fn touch_last_activity_at(
        &self,
        conversation_id: ConversationId,
        at: DateTime<Utc>,
    ) -> RepoResult<()>;
}
