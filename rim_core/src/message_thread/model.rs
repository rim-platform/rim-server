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

impl MessageThread {
    pub fn has_root_message(&self) -> bool {
        self.first_message_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageThread {
    pub conversation_id: ConversationId,
}
