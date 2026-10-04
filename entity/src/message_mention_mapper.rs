use crate::message_mention;
use rim_core::message_mention::model::MessageMention;

impl From<message_mention::Model> for MessageMention {
    fn from(m: message_mention::Model) -> Self {
        Self {
            message_id: m.message_id.into(),
            mentioned_user_id: m.mentioned_user_id.into(),
            created_at: m.created_at.into(),
        }
    }
}
