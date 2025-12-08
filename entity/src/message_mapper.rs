use rim_core::message::model::{Message};
use crate::{message};

impl From<message::Model> for Message {
    fn from(m: message::Model) -> Self {
        Self {
            id: m.id.into(),
            is_deleted: m.is_deleted,
            has_attachments: m.has_attachments,
            message_thread_id: m.message_thread_id.into(),
            parent_message_id: m.parent_message_id.map(Into::into),
            sent_at: m.sent_at.into(),
            sent_by: m.sent_by.into(),
            text: m.text,
        }
    }
}
