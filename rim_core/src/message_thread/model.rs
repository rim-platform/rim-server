use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(MessageThreadId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageThread {
    pub id: MessageThreadId,
    pub conversation_id: ConversationId,
    pub first_message_id: Option<MessageId>,
    pub created_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageThread {
    pub conversation_id: ConversationId,
}
