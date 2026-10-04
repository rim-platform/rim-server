use crate::message::model::MessageId;
use crate::model_id;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(MessageReactionId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageReaction {
    pub id: MessageReactionId,
    pub message_id: MessageId,
    pub user_id: UserId,
    pub emoji: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageReaction {
    pub message_id: MessageId,
    pub user_id: UserId,
    pub emoji: String,
}
