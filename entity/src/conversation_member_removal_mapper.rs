use crate::conversation_member_removal;
use rim_core::conversation_member::model::MemberRemoval;

impl From<conversation_member_removal::Model> for MemberRemoval {
    fn from(m: conversation_member_removal::Model) -> Self {
        Self {
            id: m.id.into(),
            conversation_id: m.conversation_id.into(),
            user_id: m.user_id.into(),
            removed_by: m.removed_by.into(),
            removed_at: m.removed_at.into(),
        }
    }
}
