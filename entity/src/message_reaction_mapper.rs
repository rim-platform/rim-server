use crate::message_reaction;
use rim_core::message_reaction::model::MessageReaction;

impl From<message_reaction::Model> for MessageReaction {
    fn from(m: message_reaction::Model) -> Self {
        Self {
            id: m.id.into(),
            message_id: m.message_id.into(),
            user_id: m.user_id.into(),
            emoji: m.emoji,
            created_at: m.created_at.into(),
        }
    }
}
