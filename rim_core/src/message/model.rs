use crate::message_attachment::model::NewMessageAttachment;
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(MessageId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub message_thread_id: MessageThreadId,
    pub parent_message_id: Option<MessageId>,
    pub sent_at: DateTime<Utc>,
    pub sent_by: UserId,
    pub text: Option<String>,
    pub is_deleted: bool,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub text: Option<String>,
    pub sent_by: UserId,
    pub message_thread_id: MessageThreadId,
    pub parent_message_id: Option<MessageId>,
    pub attachments: Vec<NewMessageAttachment>,
}
