use crate::message::model::MessageId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageMention {
    pub message_id: MessageId,
    pub mentioned_user_id: UserId,
    pub created_at: DateTime<Utc>,
}
