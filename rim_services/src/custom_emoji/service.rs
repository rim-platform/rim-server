use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::custom_emoji::command::{
    CreateCustomEmojiCmd, CustomEmojiImageQuery, DeleteCustomEmojiCmd, ListCustomEmojisQuery,
};
use crate::custom_emoji::dto::CustomEmojiDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CustomEmojiService: Send + Sync {
    async fn create(&self, cmd: CreateCustomEmojiCmd) -> ServiceResult<CustomEmojiDto>;

    async fn delete(&self, cmd: DeleteCustomEmojiCmd) -> ServiceResult<()>;

    async fn list(&self, query: ListCustomEmojisQuery) -> ServiceResult<Vec<CustomEmojiDto>>;

    /// The stored image of an emoji, for a member of its workspace.
    ///
    /// # Errors
    /// `NotFound` for an unknown emoji and for a caller who is not a member of its workspace.
    async fn image(&self, query: CustomEmojiImageQuery) -> ServiceResult<Vec<u8>>;
}
