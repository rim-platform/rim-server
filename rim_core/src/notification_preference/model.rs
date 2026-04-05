use crate::conversation::model::ConversationId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreference {
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
}
