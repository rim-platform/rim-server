use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::notification::model::NotificationLevel;
use crate::notification_preference::model::NotificationPreference;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait NotificationPreferenceRepository: Send + Sync {
    /// Mute a conversation. With `muted_until` the mute expires then (a `NONE` level override is
    /// cleared so the mute really ends; any other level stays); without it the mute is
    /// permanent, stored as level `NONE` with no `muted_until`.
    async fn set_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        muted_until: Option<DateTime<Utc>>,
    ) -> RepoResult<NotificationPreference>;

    /// Clear a mute: `muted_until` and a `NONE` level override (any other level stays). The row
    /// is deleted once it carries nothing.
    async fn remove_mute(&self, user_id: UserId, conversation_id: ConversationId)
    -> RepoResult<()>;

    /// Muted now: a timed mute in effect, or level `NONE`.
    async fn is_muted(&self, user_id: UserId, conversation_id: ConversationId) -> RepoResult<bool>;

    /// Preferences that mute: level `NONE`, or any `muted_until` (expired ones included, as
    /// before Track 1.3).
    async fn get_muted_conversations(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<NotificationPreference>>;

    /// Set (or with `None` clear) the level override, leaving `muted_until` alone. The row is
    /// deleted once it carries nothing.
    async fn set_level(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        level: Option<NotificationLevel>,
    ) -> RepoResult<Option<NotificationPreference>>;

    /// The preference of one user for one conversation, if a row exists.
    async fn get_preference(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<NotificationPreference>>;

    /// The preferences of the given users for one conversation (users without a row are absent).
    async fn list_for_conversation(
        &self,
        conversation_id: ConversationId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<NotificationPreference>>;
}
