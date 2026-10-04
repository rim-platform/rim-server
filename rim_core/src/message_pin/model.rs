use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePin {
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub pinned_by: UserId,
    pub pinned_at: DateTime<Utc>,
}
