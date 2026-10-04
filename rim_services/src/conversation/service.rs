use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::conversation::command::{
    AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery, ListChannelsQuery,
    ListInboxConversationsQuery, RemoveConversationMemberCmd, UpdateConversationTopicCmd,
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

    async fn add_member(&self, cmd: AddConversationMemberCmd) -> ServiceResult<ConversationDto>;

    async fn remove_member(
        &self,
        cmd: RemoveConversationMemberCmd,
    ) -> ServiceResult<ConversationDto>;

    async fn update_topic(&self, cmd: UpdateConversationTopicCmd)
    -> ServiceResult<ConversationDto>;
}
