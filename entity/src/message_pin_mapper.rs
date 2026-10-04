use crate::message_pin;
use rim_core::message_pin::model::MessagePin;

impl From<message_pin::Model> for MessagePin {
    fn from(m: message_pin::Model) -> Self {
        Self {
            message_id: m.message_id.into(),
            conversation_id: m.conversation_id.into(),
            pinned_by: m.pinned_by.into(),
            pinned_at: m.pinned_at.into(),
        }
    }
}
