use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::custom_emoji::model::{CustomEmoji, CustomEmojiId, NewCustomEmoji};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CustomEmojiRepository: Send + Sync {
    async fn create(&self, emoji: NewCustomEmoji) -> RepoResult<CustomEmoji>;

    async fn delete(&self, id: CustomEmojiId) -> RepoResult<()>;

    async fn find_by_id(&self, id: CustomEmojiId) -> RepoResult<Option<CustomEmoji>>;

    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<CustomEmoji>>;
}
