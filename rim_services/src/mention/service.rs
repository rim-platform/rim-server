use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::mention::command::MyMentionsQuery;
use crate::message::dto::MessageDto;
use rim_core::message::model::MessageId;
use rim_core::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MentionService: Send + Sync {
    async fn get_my_mentions(&self, query: MyMentionsQuery) -> ServiceResult<Vec<MessageDto>>;

    async fn get_mentions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> ServiceResult<HashMap<MessageId, Vec<UserId>>>;
}
