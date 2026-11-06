use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::message::model::{Message, MessageId};
use crate::message_mention::model::MessageMention;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageMentionRepository: Send + Sync {
    async fn create_mentions(&self, message_id: MessageId, user_ids: &[UserId]) -> RepoResult<()>;

    async fn delete_mentions_for_message(&self, message_id: MessageId) -> RepoResult<()>;

    async fn get_mentions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<UserId>>>;

    /// `user_id`'s mentions with their messages, oldest first by `(created_at, message_id)`: those
    /// strictly after `after`, or without it those made at or after `since`. A deleted message and
    /// a conversation the user is no longer a member of are left out.
    async fn list_since(
        &self,
        user_id: UserId,
        after: Option<(DateTime<Utc>, MessageId)>,
        since: DateTime<Utc>,
        limit: u64,
    ) -> RepoResult<Vec<(MessageMention, Message)>>;
}
