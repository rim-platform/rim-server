use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::notification::model::{
    DigestCandidate, NewUserNotification, UserNotification, UserNotificationId,
    UserNotificationSettings,
};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// Cursor for [`NotificationRepository::list_notifications`]: the standard `(created_at, id)`
/// tuple, newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserNotificationCursor {
    pub created_at: DateTime<Utc>,
    pub notification_id: UserNotificationId,
}

/// Parameters of [`NotificationRepository::list_notifications`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListUserNotificationsParams {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub unread_only: bool,
    pub limit: u32,
    pub dir: PageDir,
    pub cursor: Option<UserNotificationCursor>,
}

/// Notification settings and the notification inbox. Authorization lives in the
/// service layer: every method trusts the ids it is given.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait NotificationRepository: Send + Sync {
    /// The stored settings of `user_id` in `server_id`, if any.
    async fn get_settings(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> RepoResult<Option<UserNotificationSettings>>;

    /// Stored settings of the given users in `server_id` (users without a row are absent).
    async fn get_settings_for_users(
        &self,
        server_id: ServerId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<UserNotificationSettings>>;

    /// Insert or replace the settings row for `(user_id, server_id)`.
    async fn upsert_settings(
        &self,
        settings: UserNotificationSettings,
    ) -> RepoResult<UserNotificationSettings>;

    /// Insert notifications, skipping any `(user_id, message_id)` that already has a row.
    /// Returns the rows actually inserted.
    async fn create_notifications(
        &self,
        notifications: &[NewUserNotification],
        created_at: DateTime<Utc>,
    ) -> RepoResult<Vec<UserNotification>>;

    /// A user's notifications in one workspace, ordered `created_at DESC, id DESC`, with the
    /// standard cursor semantics (DESC endpoint).
    async fn list_notifications(
        &self,
        params: ListUserNotificationsParams,
    ) -> RepoResult<Page<UserNotification, UserNotificationCursor>>;

    /// Unread notifications of a user in one workspace.
    async fn unread_count(&self, user_id: UserId, server_id: ServerId) -> RepoResult<u64>;

    /// Mark the given notifications of the user in the workspace read (ids of other users or
    /// workspaces, and already-read rows, are left alone). Returns rows changed.
    async fn mark_read_by_ids(
        &self,
        user_id: UserId,
        server_id: ServerId,
        ids: &[UserNotificationId],
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Mark every unread notification of the user in the workspace created at or before
    /// `before` read. Returns rows changed.
    async fn mark_read_before(
        &self,
        user_id: UserId,
        server_id: ServerId,
        before: DateTime<Utc>,
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Mark the user's unread notifications for messages of `conversation_id` sent at or before
    /// `up_to` read (reading a conversation). Returns rows changed.
    async fn mark_read_for_conversation(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        up_to: DateTime<Utc>,
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Delete the unread notifications of a message (it was deleted); read ones stay.
    async fn delete_unread_for_message(&self, message_id: MessageId) -> RepoResult<u64>;

    /// Delete every notification of the user in the workspace, read or not (they lost access
    /// to it). Returns rows deleted.
    async fn delete_for_user_in_server(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> RepoResult<u64>;

    /// Delete every notification of the user for the conversation, read or not (they lost
    /// access to it). Returns rows deleted.
    async fn delete_for_user_in_conversation(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<u64>;

    /// Delete notifications created before `cutoff` (retention).
    async fn delete_created_before(&self, cutoff: DateTime<Utc>) -> RepoResult<u64>;

    /// Users with a digest frequency other than `off` whose oldest unread, un-emailed
    /// notification is at least one digest interval old (hourly: created at or before
    /// `hourly_cutoff`; daily: at or before `daily_cutoff`) and whose last digest, if any, is at
    /// least as old. One row per `(user, workspace)`. Users without an email address (key-only
    /// accounts) are left out: there is nowhere to send their digest.
    async fn list_digest_candidates(
        &self,
        hourly_cutoff: DateTime<Utc>,
        daily_cutoff: DateTime<Utc>,
    ) -> RepoResult<Vec<DigestCandidate>>;

    /// Unread, un-emailed notifications of a user in a workspace, oldest first, at most `limit`.
    async fn list_pending_digest(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
    ) -> RepoResult<Vec<UserNotification>>;

    /// Set `emailed_at` on the given notifications. Returns rows changed.
    async fn mark_emailed(
        &self,
        ids: &[UserNotificationId],
        emailed_at: DateTime<Utc>,
    ) -> RepoResult<u64>;
}
