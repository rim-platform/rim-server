use crate::conversation_member;
use rim_core::conversation_member::model::ConversationMember;

impl From<conversation_member::Model> for ConversationMember {
    fn from(m: conversation_member::Model) -> Self {
        Self {
            id: m.id.into(),
            conversation_id: m.conversation_id.into(),
            user_id: m.user_id.into(),
            joined_at: m.joined_at.into(),
        }
    }
}
