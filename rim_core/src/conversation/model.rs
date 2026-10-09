use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
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
    /// Only meaningful for channels: a private channel is visible to its members only. Always
    /// `false` for DMs and groups (they are member-only by construction). Fixed at creation.
    pub is_private: bool,
    /// Set while the conversation is archived (read-only, hidden from default channel lists).
    pub archived_at: Option<DateTime<Utc>>,
    /// Who created the conversation. `None` for rows that predate the column or whose creator
    /// was deleted.
    pub created_by: Option<UserId>,
    /// Slow mode (Track 1.2): minimum seconds between two messages of the same non-moderator
    /// member in this conversation. `0` means off. Only channels can have it set.
    pub slow_mode_seconds: i32,
    /// When the conversation became end-to-end encrypted; never cleared once set. Only DMs and
    /// groups are encrypted.
    pub encrypted_at: Option<DateTime<Utc>>,
}

impl Conversation {
    /// A channel every workspace member can read without being a conversation member.
    pub fn is_public_channel(&self) -> bool {
        self.conversation_type == ConversationType::Channel && !self.is_private
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted_at.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversation {
    pub server_id: ServerId,
    pub conversation_type: ConversationType,
    pub title: Option<String>,
    pub is_private: bool,
    pub created_by: Option<UserId>,
    /// Created encrypted: `encrypted_at` is set to the creation time.
    pub encrypted: bool,
}
