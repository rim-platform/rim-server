use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::custom_emoji::command::{CreateCustomEmojiCmd, DeleteCustomEmojiCmd, ListCustomEmojisQuery};
use crate::custom_emoji::dto::CustomEmojiDto;
use crate::ServiceResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CustomEmojiService: Send + Sync {
    async fn create(&self, cmd: CreateCustomEmojiCmd) -> ServiceResult<CustomEmojiDto>;

    async fn delete(&self, cmd: DeleteCustomEmojiCmd) -> ServiceResult<()>;

    async fn list(&self, query: ListCustomEmojisQuery) -> ServiceResult<Vec<CustomEmojiDto>>;
}
