use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::message::model::MessageId;
use crate::message_reaction::model::{MessageReaction, NewMessageReaction};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageReactionRepository: Send + Sync {
    async fn add_reaction(&self, reaction: NewMessageReaction) -> RepoResult<MessageReaction>;

    async fn remove_reaction(
        &self,
        message_id: MessageId,
        user_id: UserId,
        emoji: &str,
    ) -> RepoResult<()>;

    async fn find_reaction(
        &self,
        message_id: MessageId,
        user_id: UserId,
        emoji: &str,
    ) -> RepoResult<Option<MessageReaction>>;

    async fn get_reactions_for_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Vec<MessageReaction>>;

    async fn get_reactions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<MessageReaction>>>;
}
