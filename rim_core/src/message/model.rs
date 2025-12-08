use chrono::{DateTime, Utc};
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::user::model::UserId;

model_id!(MessageId);

pub struct Message {
    pub id: MessageId,
    pub message_thread_id: MessageThreadId,
    pub parent_message_id: Option<MessageId>,
    pub sent_at: DateTime<Utc>,
    pub sent_by: UserId,
    pub text: String,
    pub is_deleted: bool,
    pub has_attachments: bool,
}
