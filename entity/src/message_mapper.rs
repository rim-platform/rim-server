use crate::message;
use rim_core::message::model::{Message, MessageEncryption};

impl From<message::Model> for Message {
    fn from(m: message::Model) -> Self {
        Self {
            conversation_id: m.conversation_id.into(),
            id: m.id.into(),
            is_deleted: m.is_deleted,
            has_attachments: m.has_attachments,
            encryption: encryption(&m),
            thread_id: m.thread_id.map(Into::into),
            parent_message_id: m.parent_message_id.map(Into::into),
            forwarded_from_message_id: m.forwarded_from_message_id.map(Into::into),
            sent_at: m.sent_at.into(),
            edited_at: m.edited_at.map(Into::into),
            sent_by: m.sent_by.into(),
            text: m.text,
            flagged_at: m.flagged_at.map(Into::into),
        }
    }
}

/// The envelope of an encrypted row; `None` for plaintext rows and for rows whose envelope
/// columns are incomplete.
fn encryption(m: &message::Model) -> Option<MessageEncryption> {
    if !m.encrypted {
        return None;
    }
    Some(MessageEncryption {
        incarnation: m.mls_incarnation?,
        epoch: m.mls_epoch?,
        sender_device_key_id: m.sender_device_key_id.map(Into::into),
        ciphertext: m.mls_ciphertext.clone(),
        franking_commitment: m.franking_commitment.clone()?,
        franking_receipt: m.franking_receipt.clone()?,
        franking_key_id: m.franking_key_id?,
    })
}
