use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::message::model::MessageId;
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
}
