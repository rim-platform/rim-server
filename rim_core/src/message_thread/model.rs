use chrono::{DateTime, Utc};
use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::model_id;

model_id!(MessageThreadId);

pub struct MessageThread {
    pub id: MessageThreadId,
    pub conversation_id: ConversationId,
    pub first_message_id: Option<MessageId>,
    pub created_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
}