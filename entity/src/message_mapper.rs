use crate::message;
use rim_core::message::model::Message;

impl From<message::Model> for Message {
    fn from(m: message::Model) -> Self {
        Self {
            conversation_id: m.conversation_id.into(),
            id: m.id.into(),
            is_deleted: m.is_deleted,
            has_attachments: m.has_attachments,
            thread_id: m.thread_id.map(Into::into),
            parent_message_id: m.parent_message_id.map(Into::into),
            sent_at: m.sent_at.into(),
            sent_by: m.sent_by.into(),
            text: m.text,
        }
    }
}
