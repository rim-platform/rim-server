use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::conversation::command::{
    AddConversationMembersCmd, ArchiveConversationCmd, CreateConversationCmd, GetConversationQuery,
    JoinChannelCmd, LeaveConversationCmd, ListChannelsQuery, ListInboxConversationsQuery,
    RemoveConversationMemberCmd, UpdateConversationTopicCmd,
};
use crate::conversation::dto::{ChannelPageCursor, ConversationDto, InboxPageCursor};
use rim_core::conversation::model::ConversationId;
use rim_core::repository::pagination::Page;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationService: Send + Sync {
    async fn create_conversation(
        &self,
        cmd: CreateConversationCmd,
    ) -> ServiceResult<ConversationDto>;

    async fn get_conversation(&self, query: GetConversationQuery)
    -> ServiceResult<ConversationDto>;

    async fn get_conversations_by_ids(
        &self,
        ids: &[ConversationId],
    ) -> ServiceResult<Vec<ConversationDto>>;

    async fn list_inbox_conversations(
        &self,
        query: ListInboxConversationsQuery,
    ) -> ServiceResult<Page<ConversationDto, InboxPageCursor>>;

    async fn list_channels(
        &self,
        query: ListChannelsQuery,
    ) -> ServiceResult<Page<ConversationDto, ChannelPageCursor>>;

    /// Add workspace members to a channel or group the actor belongs to. Already-member targets
    /// are skipped. Emits `MemberAddedToConversation` per added user.
    async fn add_conversation_members(
        &self,
        cmd: AddConversationMembersCmd,
    ) -> ServiceResult<ConversationDto>;

    /// Owner/Moderator remove anyone; others only themselves. Emits
    /// `MemberRemovedFromConversation`.
    async fn remove_conversation_member(
        &self,
        cmd: RemoveConversationMemberCmd,
    ) -> ServiceResult<()>;

    /// Join a public, unarchived channel. Idempotent.
    async fn join_channel(&self, cmd: JoinChannelCmd) -> ServiceResult<ConversationDto>;

    /// Leave a channel or group (not a DM).
    async fn leave_conversation(&self, cmd: LeaveConversationCmd) -> ServiceResult<()>;

    /// Archive or unarchive a channel (Owner/Moderator). Emits `ConversationUpdated`.
    async fn set_conversation_archived(
        &self,
        cmd: ArchiveConversationCmd,
    ) -> ServiceResult<ConversationDto>;

    async fn update_topic(&self, cmd: UpdateConversationTopicCmd)
    -> ServiceResult<ConversationDto>;
}
