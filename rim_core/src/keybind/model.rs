use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;

use crate::model_id;
use crate::server::model::WorkspaceId;
use crate::user::model::UserId;

model_id!(KeybindId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindScope {
    User,
    Workspace,
}

impl KeybindScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

impl fmt::Display for KeybindScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KeybindScope {
    type Err = ParseKeybindScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "workspace" => Ok(Self::Workspace),
            _ => Err(ParseKeybindScopeError::new(s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeybindScopeError {
    input: String,
}

impl ParseKeybindScopeError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseKeybindScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown keybind scope: {}", self.input)
    }
}

impl std::error::Error for ParseKeybindScopeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindContext {
    Global,
    Composer,
    MessageList,
    ThreadView,
    ConversationView,
    Panel,
    CommandPalette,
}

impl KeybindContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Composer => "composer",
            Self::MessageList => "message_list",
            Self::ThreadView => "thread_view",
            Self::ConversationView => "conversation_view",
            Self::Panel => "panel",
            Self::CommandPalette => "command_palette",
        }
    }
}

impl fmt::Display for KeybindContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KeybindContext {
    type Err = ParseKeybindContextError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Self::Global),
            "composer" => Ok(Self::Composer),
            "message_list" => Ok(Self::MessageList),
            "thread_view" => Ok(Self::ThreadView),
            "conversation_view" => Ok(Self::ConversationView),
            "panel" => Ok(Self::Panel),
            "command_palette" => Ok(Self::CommandPalette),
            _ => Err(ParseKeybindContextError::new(s)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeybindContextError {
    input: String,
}

impl ParseKeybindContextError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseKeybindContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown keybind context: {}", self.input)
    }
}

impl std::error::Error for ParseKeybindContextError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindSubject {
    User(UserId),
    Workspace(WorkspaceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybind {
    pub id: KeybindId,
    pub scope: KeybindScope,
    pub subject: KeybindSubject,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKeybind {
    pub scope: KeybindScope,
    pub subject: KeybindSubject,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateKeybind {
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}
