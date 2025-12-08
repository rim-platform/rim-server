use chrono::{DateTime, Utc};
use crate::model_id;
use crate::server_member::model::ServerMemberRole;

model_id!(ConversationId);

pub enum ConversationType {
    Direct,
    Group,
    Channel,
}

impl std::str::FromStr for ConversationType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
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
}
