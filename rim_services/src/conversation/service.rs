use async_trait::async_trait;

use crate::ServiceResult;
use crate::conversation::command::{
    AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery,
    ListConversationsQuery, RemoveConversationMemberCmd,
};
use crate::conversation::dto::{ConversationDto};

#[async_trait]
pub trait ConversationService: Send + Sync {
    async fn create_conversation(&self, cmd: CreateConversationCmd) -> ServiceResult<ConversationDto>;

    async fn get_conversation(&self, query: GetConversationQuery) -> ServiceResult<ConversationDto>;

    async fn list_conversations(&self, query: ListConversationsQuery) -> ServiceResult<Vec<ConversationDto>>;

    async fn add_member(&self, cmd: AddConversationMemberCmd) -> ServiceResult<ConversationDto>;

    async fn remove_member(&self, cmd: RemoveConversationMemberCmd) -> ServiceResult<ConversationDto>;
}
