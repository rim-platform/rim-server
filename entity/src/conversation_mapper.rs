use crate::{conversation, sea_orm_active_enums};
use rim_core::conversation::model::Conversation;
use rim_core::conversation::model::ConversationType;
use sea_orm::ActiveEnum;
use std::str::FromStr;

impl From<conversation::Model> for Conversation {
    fn from(m: conversation::Model) -> Self {
        Self {
            id: m.id.into(),
            title: m.title,
            created_at: m.created_at.into(),
            last_activity_at: m.last_activity_at.into(),
            conversation_type: ConversationType::from_str(
                m.conversation_type.into_value().as_str(),
            )
            .unwrap(),
            server_id: m.server_id.into(),
        }
    }
}

impl From<ConversationType> for sea_orm_active_enums::ConversationType {
    fn from(value: ConversationType) -> Self {
        match value {
            ConversationType::Channel => sea_orm_active_enums::ConversationType::Channel,
            ConversationType::Group => sea_orm_active_enums::ConversationType::Group,
            ConversationType::Direct => sea_orm_active_enums::ConversationType::Direct,
        }
    }
}
