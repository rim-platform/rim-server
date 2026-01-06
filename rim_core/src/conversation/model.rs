use crate::model_id;
use crate::server::model::ServerId;
use chrono::{DateTime, Utc};

model_id!(ConversationId);

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ConversationType {
    Direct,
    Group,
    Channel,
}

impl std::str::FromStr for ConversationType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "direct" => Ok(ConversationType::Direct),
            "group" => Ok(ConversationType::Group),
            "channel" => Ok(ConversationType::Channel),
            other => Err(anyhow::anyhow!("Unknown role: {other}")),
        }
    }
}

pub struct Conversation {
    pub id: ConversationId,
    pub conversation_type: ConversationType,
    pub created_at: DateTime<Utc>,
    pub server_id: ServerId,
}

pub struct NewConversation {
    pub server_id: ServerId,
    pub conversation_type: ConversationType,
}
