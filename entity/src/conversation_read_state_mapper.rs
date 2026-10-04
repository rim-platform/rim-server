use crate::conversation_read_state;
use rim_core::conversation_read_state::model::ConversationReadState;

impl From<conversation_read_state::Model> for ConversationReadState {
    fn from(m: conversation_read_state::Model) -> Self {
        Self {
            user_id: m.user_id.into(),
            conversation_id: m.conversation_id.into(),
            last_read_at: m.last_read_at.into(),
        }
    }
}
