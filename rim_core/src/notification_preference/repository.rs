use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::notification_preference::model::NotificationPreference;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait NotificationPreferenceRepository: Send + Sync {
    async fn set_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        muted_until: Option<DateTime<Utc>>,
    ) -> RepoResult<NotificationPreference>;

    async fn remove_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<()>;

    async fn is_muted(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<bool>;

    async fn get_muted_conversations(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<NotificationPreference>>;
}
