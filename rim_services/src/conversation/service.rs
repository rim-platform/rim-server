use async_trait::async_trait;

use crate::ServiceResult;
use crate::conversation::command::{
    AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery, ListChannelsQuery,
    ListInboxConversationsQuery, RemoveConversationMemberCmd,
};
use crate::conversation::dto::{ChannelPageCursor, ConversationDto, InboxPageCursor};
use rim_core::repository::pagination::Page;

#[async_trait]
pub trait ConversationService: Send + Sync {
    async fn create_conversation(
        &self,
        cmd: CreateConversationCmd,
    ) -> ServiceResult<ConversationDto>;

    async fn get_conversation(&self, query: GetConversationQuery)
    -> ServiceResult<ConversationDto>;

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
}
