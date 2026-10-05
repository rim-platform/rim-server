use chrono::{DateTime, Utc};

use rim_core::conversation::model::{Conversation, ConversationId};
use rim_core::message::model::Message;
use rim_core::notification::model::{
    DigestFrequency, DndPause, DndWindow, NotificationLevel, UserNotificationId,
};
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::Actor;
use crate::notification::dto::NotificationPageCursor;

#[derive(Debug, Clone)]
pub struct GetNotificationSettingsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Replaces the editable settings. Keywords: at most 20, each 2 to 40 characters after
/// trimming, stored lower-cased and deduplicated. The window's timezone must be an IANA name.
/// The manual pause (`SetDndPauseCmd`) is left alone.
#[derive(Debug, Clone)]
pub struct UpdateNotificationSettingsCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub default_level: NotificationLevel,
    pub keywords: Vec<String>,
    pub dnd: Option<DndWindow>,
    pub digest: DigestFrequency,
}

/// Turns the manual do-not-disturb pause on (indefinitely or until an instant in the future)
/// or off.
#[derive(Debug, Clone)]
pub struct SetDndPauseCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub pause: DndPause,
}

/// Membership-gated (the actor must be able to read the conversation). `level: None` clears the
/// override, so the workspace default applies.
#[derive(Debug, Clone)]
pub struct SetConversationNotificationLevelCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub level: Option<NotificationLevel>,
}

/// The actor's own level for a conversation. Reads only the actor's own preference and
/// settings, so it does not gate on access (the caller already resolved the conversation).
#[derive(Debug, Clone)]
pub struct GetConversationNotificationLevelQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct ListNotificationsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub unread_only: bool,
    pub dir: PageDir,
    pub cursor: Option<NotificationPageCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct UnreadNotificationCountQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Which notifications [`MarkNotificationsReadCmd`] marks read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkNotificationsReadTarget {
    /// These notifications (other users' ids are ignored).
    Ids(Vec<UserNotificationId>),
    /// Every unread notification created at or before the instant.
    AllBefore(DateTime<Utc>),
}

#[derive(Debug, Clone)]
pub struct MarkNotificationsReadCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub target: MarkNotificationsReadTarget,
}

/// Internal: sent by the message service after a message commits. `member_user_ids` is the
/// conversation's member list (the same list its events go to).
#[derive(Debug, Clone)]
pub struct NotifyForMessageCmd {
    pub server_id: ServerId,
    pub conversation: Conversation,
    pub message: Message,
    pub member_user_ids: Vec<UserId>,
}
