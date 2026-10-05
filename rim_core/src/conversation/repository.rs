use crate::conversation::model::{Conversation, ConversationId, NewConversation};
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

/// Which channels [`ConversationRepository::list_channels_for_user`] returns. The default lists
/// every channel the user can see (public ones plus private ones they belong to), unarchived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelListFilter {
    /// Only channels the user is a conversation member of.
    pub joined_only: bool,
    /// Include archived channels.
    pub include_archived: bool,
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationRepository: Send + Sync {
    async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;

    async fn get_by_ids(&self, ids: &[ConversationId]) -> RepoResult<Vec<Conversation>>;

    async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation>;

    async fn create_conversation_with_members(
        &self,
        conv: NewConversation,
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

    /// List the channels a user can see, in a stable order: public channels plus private channels
    /// the user is a conversation member of, narrowed by `filter` (archived channels are excluded
    /// unless `filter.include_archived`).
    ///
    /// v0 ordering: `created_at ASC, id ASC`.
    async fn list_channels_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
        filter: ChannelListFilter,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationCreatedCursor>,
    ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;

    /// Check whether a channel with the given title already exists on the server.
    async fn exists_channel_by_title(&self, server_id: ServerId, title: &str) -> RepoResult<bool>;

    /// Look up a channel conversation by its exact title within a server. Channel titles are
    /// unique per server (see `m20260322_000000_add_unique_channel_title`), so this returns at
    /// most one row. Returns `None` if no channel with that title exists (or a non-channel
    /// conversation happens to share the title — this only ever matches `ConversationType::Channel`
    /// rows). Used by the `in:#channel` search-syntax operator to resolve a channel name to an id.
    async fn get_channel_by_title(
        &self,
        server_id: ServerId,
        title: &str,
    ) -> RepoResult<Option<Conversation>>;

    /// List all conversation IDs that the user can access in a server.
    /// Includes: public channels, archived or not (server membership is sufficient) + private
    /// channels, DMs and groups where the user is a conversation member.
    async fn list_accessible_conversation_ids(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>>;

    /// Public, unarchived channels of a server — the channels a new workspace member joins
    /// automatically (see `add_workspace_member` in `rim_infra_services`).
    async fn list_public_channel_ids(&self, server_id: ServerId)
    -> RepoResult<Vec<ConversationId>>;

    /// Conversations in a server the user is a `conversation_member` of: joined public channels,
    /// private channels, DMs and groups (not public channels they merely can read). Scopes the
    /// unread counts.
    async fn list_joined_conversation_ids(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>>;

    /// Update the conversation activity timestamp.
    /// Intended to be called when a new message is persisted.
    async fn touch_last_activity_at(
        &self,
        conversation_id: ConversationId,
        at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Update the conversation topic.
    async fn update_topic(
        &self,
        conversation_id: ConversationId,
        topic: Option<String>,
    ) -> RepoResult<Conversation>;

    /// Archive (`Some(at)`) or unarchive (`None`) a conversation.
    async fn set_archived(
        &self,
        conversation_id: ConversationId,
        archived_at: Option<DateTime<Utc>>,
    ) -> RepoResult<Conversation>;
}
