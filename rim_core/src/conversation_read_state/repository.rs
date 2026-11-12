use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// One conversation's unread messages for a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationUnread {
    pub conversation_id: ConversationId,
    pub count: i64,
    /// Messages sent after this time count as unread.
    pub unread_after: DateTime<Utc>,
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationReadStateRepository: Send + Sync {
    async fn upsert(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        last_read_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    async fn get_unread_counts(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<Vec<ConversationUnread>>;

    async fn mark_all_read(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
        read_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Moves the read marker back to `to`, the only path that moves it backward. Nothing becomes
    /// less unread: a marker already at or before `to` stays where it is, and without a marker one
    /// is written only when `to` is before the baseline the unread count starts from (when the
    /// user joined, else when the conversation was created).
    ///
    /// Returns whether the marker was written.
    ///
    /// # Errors
    /// Storage errors.
    async fn rewind(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        to: DateTime<Utc>,
    ) -> RepoResult<bool>;
}
