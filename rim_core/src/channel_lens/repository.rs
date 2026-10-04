use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::channel_lens::model::{ChannelLens, ChannelLensId, NewChannelLens};
use crate::conversation::model::ConversationId;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ChannelLensRepository: Send + Sync {
    async fn create(&self, lens: NewChannelLens) -> RepoResult<ChannelLens>;

    async fn get_by_id(&self, id: ChannelLensId) -> RepoResult<Option<ChannelLens>>;

    async fn update(&self, lens: &ChannelLens) -> RepoResult<ChannelLens>;

    async fn delete(&self, id: ChannelLensId) -> RepoResult<()>;

    /// List lenses visible to a user in a conversation: owned by the user + shared.
    async fn list_for_conversation(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<Vec<ChannelLens>>;

    /// Count how many lenses a user owns in a conversation (for limit enforcement).
    async fn count_user_lenses(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<u64>;
}
