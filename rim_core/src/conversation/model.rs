use crate::model_id;
use crate::server::model::ServerId;
use chrono::{DateTime, Utc};
use std::fmt;

model_id!(ConversationId);

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConversationType {
    Direct,
    Group,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseConversationTypeError {
    input: String,
}

impl ParseConversationTypeError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseConversationTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown conversation type: {}", self.input)
    }
}

impl std::error::Error for ParseConversationTypeError {}

impl std::str::FromStr for ConversationType {
    type Err = ParseConversationTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "direct" => Ok(ConversationType::Direct),
            "group" => Ok(ConversationType::Group),
            "channel" => Ok(ConversationType::Channel),
            other => Err(ParseConversationTypeError::new(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: ConversationId,
    pub conversation_type: ConversationType,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub server_id: ServerId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversation {
    pub server_id: ServerId,
    pub conversation_type: ConversationType,
}
