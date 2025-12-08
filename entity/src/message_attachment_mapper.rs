use rim_core::message_attachment::model::{MessageAttachment};
use crate::{message_attachment};

impl From<message_attachment::Model> for MessageAttachment {
    fn from(m: message_attachment::Model) -> Self {
        Self {
            id: m.id.into(),
            message_id: m.message_id.into(),
            metadata: m.metadata,
            file_name: m.file_name,
            file_size: m.file_size,
            mime_type: m.mime_type,
            storage_url: m.storage_url,
        }
    }
}
