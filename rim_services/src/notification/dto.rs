use chrono::{DateTime, Utc};

use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::notification::model::{
    DigestFrequency, DndPause, DndWindow, NotificationLevel, NotificationReason, UserNotification,
    UserNotificationId, UserNotificationSettings,
};
use rim_core::notification::repository::UserNotificationCursor;
use rim_core::server::model::ServerId;

/// A user's notification settings in one workspace (defaults when never saved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationSettingsDto {
    pub server_id: ServerId,
    pub default_level: NotificationLevel,
    pub keywords: Vec<String>,
    pub dnd: Option<DndWindow>,
    pub dnd_pause: DndPause,
    pub digest: DigestFrequency,
}

impl From<UserNotificationSettings> for NotificationSettingsDto {
    fn from(s: UserNotificationSettings) -> Self {
        Self {
            server_id: s.server_id,
            default_level: s.default_level,
            keywords: s.keywords,
            dnd: s.dnd,
            dnd_pause: s.dnd_pause,
            digest: s.digest,
        }
    }
}

/// The viewer's notification level for one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationNotificationLevelDto {
    pub conversation_id: ConversationId,
    /// The per-conversation override; `None` inherits the workspace default.
    pub level: Option<NotificationLevel>,
    /// What applies now: `NONE` during a timed mute, else the override, else the workspace
    /// default.
    pub effective_level: NotificationLevel,
    pub muted_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDto {
    pub id: UserNotificationId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reason: NotificationReason,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

impl From<UserNotification> for NotificationDto {
    fn from(n: UserNotification) -> Self {
        Self {
            id: n.id,
            server_id: n.server_id,
            conversation_id: n.conversation_id,
            message_id: n.message_id,
            reason: n.reason,
            created_at: n.created_at,
            read_at: n.read_at,
        }
    }
}

/// `(created_at, id)` cursor of the notification list (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPageCursor {
    pub created_at: DateTime<Utc>,
    pub notification_id: UserNotificationId,
}

impl From<UserNotificationCursor> for NotificationPageCursor {
    fn from(c: UserNotificationCursor) -> Self {
        Self {
            created_at: c.created_at,
            notification_id: c.notification_id,
        }
    }
}

impl From<NotificationPageCursor> for UserNotificationCursor {
    fn from(c: NotificationPageCursor) -> Self {
        Self {
            created_at: c.created_at,
            notification_id: c.notification_id,
        }
    }
}
