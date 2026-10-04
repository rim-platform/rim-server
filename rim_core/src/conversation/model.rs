use crate::model_id;
use crate::server::model::ServerId;
use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;

model_id!(ConversationId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ConversationType {
    Direct,
    Group,
    Channel,
}

impl ConversationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "Direct",
            Self::Group => "Group",
            Self::Channel => "Channel",
        }
    }
}

impl fmt::Display for ConversationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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

impl FromStr for ConversationType {
    type Err = ParseConversationTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Direct" => Ok(ConversationType::Direct),
            "Group" => Ok(ConversationType::Group),
            "Channel" => Ok(ConversationType::Channel),
            other => Err(ParseConversationTypeError::new(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: ConversationId,
    pub conversation_type: ConversationType,
    pub title: Option<String>,
    pub topic: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub server_id: ServerId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversation {
    pub server_id: ServerId,
    pub conversation_type: ConversationType,
    pub title: Option<String>,
}
