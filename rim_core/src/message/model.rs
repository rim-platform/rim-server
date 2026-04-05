use crate::conversation::model::ConversationId;
use crate::message_attachment::model::NewMessageAttachment;
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(MessageId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub thread_id: Option<MessageThreadId>,
    pub parent_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub sent_at: DateTime<Utc>,
    pub sent_by: UserId,
    pub text: Option<String>,
    pub is_deleted: bool,
    pub has_attachments: bool,
}

impl Message {
    pub fn is_timeline_message(&self) -> bool {
        self.thread_id.is_none()
    }

    pub fn is_thread_reply(&self) -> bool {
        self.thread_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub conversation_id: ConversationId,
    pub text: Option<String>,
    pub sent_by: UserId,
    pub thread_id: Option<MessageThreadId>,
    pub parent_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub attachments: Vec<NewMessageAttachment>,
}

impl NewMessage {
    pub fn is_timeline_message(&self) -> bool {
        self.thread_id.is_none()
    }

    pub fn is_thread_reply(&self) -> bool {
        self.thread_id.is_some()
    }
}
