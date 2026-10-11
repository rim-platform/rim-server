use chrono::{DateTime, Utc};

use crate::conversation::model::ConversationId;
use crate::notification::model::NotificationLevel;
use crate::user::model::UserId;

/// A user's notification preference for one conversation.
///
/// - `level`: the per-conversation override; `None` inherits the workspace default.
/// - `muted_until`: a timed mute (an explicit `NONE` that expires). A permanent mute is stored as
///   `level = Some(None)` without `muted_until`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreference {
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
    pub level: Option<NotificationLevel>,
}

impl NotificationPreference {
    /// Whether a timed mute is in effect at `now`.
    pub fn is_timed_muted_at(&self, now: DateTime<Utc>) -> bool {
        self.muted_until.is_some_and(|until| until > now)
    }

    /// Whether the conversation is muted at `now`: a timed mute in effect, or level `NONE`.
    pub fn is_muted_at(&self, now: DateTime<Utc>) -> bool {
        self.is_timed_muted_at(now) || self.level == Some(NotificationLevel::None)
    }
}
