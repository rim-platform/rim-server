use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::message::model::MessageId;
use rim_core::repository::pagination::Page;

use crate::ServiceResult;
use crate::notification::command::{
    GetConversationNotificationLevelQuery, GetNotificationSettingsQuery, ListNotificationsQuery,
    MarkNotificationsReadCmd, NotifyForMessageCmd, SetConversationNotificationLevelCmd,
    SetDndPauseCmd, UnreadNotificationCountQuery, UpdateNotificationSettingsCmd,
};
use crate::notification::dto::{
    ConversationNotificationLevelDto, NotificationDto, NotificationPageCursor,
    NotificationSettingsDto,
};

/// Notification rules and the notification inbox. Every actor-facing method
/// requires workspace membership (`NotFound`/`PermissionDenied` otherwise) and only ever touches
/// the actor's own settings and notifications.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait NotificationService: Send + Sync {
    async fn get_settings(
        &self,
        query: GetNotificationSettingsQuery,
    ) -> ServiceResult<NotificationSettingsDto>;

    async fn update_settings(
        &self,
        cmd: UpdateNotificationSettingsCmd,
    ) -> ServiceResult<NotificationSettingsDto>;

    async fn set_dnd_pause(&self, cmd: SetDndPauseCmd) -> ServiceResult<NotificationSettingsDto>;

    async fn set_conversation_level(
        &self,
        cmd: SetConversationNotificationLevelCmd,
    ) -> ServiceResult<ConversationNotificationLevelDto>;

    async fn get_conversation_level(
        &self,
        query: GetConversationNotificationLevelQuery,
    ) -> ServiceResult<ConversationNotificationLevelDto>;

    /// Ordered `created_at DESC, id DESC` with the standard cursor semantics.
    async fn list_notifications(
        &self,
        query: ListNotificationsQuery,
    ) -> ServiceResult<Page<NotificationDto, NotificationPageCursor>>;

    async fn unread_count(&self, query: UnreadNotificationCountQuery) -> ServiceResult<u64>;

    /// Returns how many notifications changed from unread to read.
    async fn mark_read(&self, cmd: MarkNotificationsReadCmd) -> ServiceResult<u64>;

    /// Internal (not exposed over GraphQL): decide who is told about a committed message, store
    /// their notifications and emit one `NotificationDue` per recipient. Returns the number of
    /// notifications created. Callers treat errors as best-effort (log, never fail the send).
    async fn notify_for_message(&self, cmd: NotifyForMessageCmd) -> ServiceResult<usize>;

    /// Internal: a message was deleted; drop its unread notifications (read ones stay).
    async fn forget_message(&self, message_id: MessageId) -> ServiceResult<u64>;
}
