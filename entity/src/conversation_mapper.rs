use std::str::FromStr;
use sea_orm::ActiveEnum;
use rim_core::conversation::model::{Conversation};
use rim_core::conversation::model::ConversationType;
use crate::{conversation, sea_orm_active_enums};

impl From<conversation::Model> for Conversation {
    fn from(m: conversation::Model) -> Self {
        Self {
            id: m.id.into(),
            created_at: m.created_at.into(),
            conversation_type: ConversationType::from_str(
                m.conversation_type
                    .into_value()
                    .as_str()
            ).unwrap(),
        }
    }
}

impl Into<sea_orm_active_enums::ConversationType> for ConversationType {
    fn into(self) -> sea_orm_active_enums::ConversationType {
        match self {
            ConversationType::Channel => sea_orm_active_enums::ConversationType::Channel,
            ConversationType::Group => sea_orm_active_enums::ConversationType::Group,
            ConversationType::Direct => sea_orm_active_enums::ConversationType::Direct,
        }
    }
}