use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use std::collections::HashMap;

use crate::ServiceResult;
use crate::message_reaction::command::{
    GetMessageReactionsQuery, GetReactionsForMessagesQuery, ToggleReactionCmd,
};
use crate::message_reaction::dto::ReactionGroupDto;
use rim_core::message::model::MessageId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageReactionService: Send + Sync {
    async fn toggle_reaction(&self, cmd: ToggleReactionCmd)
    -> ServiceResult<Vec<ReactionGroupDto>>;

    async fn get_reactions(
        &self,
        query: GetMessageReactionsQuery,
    ) -> ServiceResult<Vec<ReactionGroupDto>>;

    async fn get_reactions_for_messages(
        &self,
        query: GetReactionsForMessagesQuery,
    ) -> ServiceResult<HashMap<MessageId, Vec<ReactionGroupDto>>>;
}
