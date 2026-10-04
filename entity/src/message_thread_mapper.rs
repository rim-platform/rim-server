use crate::message_thread;
use rim_core::message_thread::model::MessageThread;

impl From<message_thread::Model> for MessageThread {
    fn from(m: message_thread::Model) -> Self {
        Self {
            id: m.id.into(),
            created_at: m.created_at.into(),
            conversation_id: m.conversation_id.into(),
            first_message_id: m.first_message_id.map(Into::into),
            last_message_at: m.last_message_at.into(),
        }
    }
}
