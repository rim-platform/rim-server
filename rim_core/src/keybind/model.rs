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

impl fmt::Display for KeybindScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        })
    }
}

impl FromStr for KeybindScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "workspace" => Ok(Self::Workspace),
            _ => Err(format!("invalid keybind scope `{s}`")),
        }
    }
}

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

impl fmt::Display for KeybindContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Global => "global",
            Self::Composer => "composer",
            Self::MessageList => "message_list",
            Self::ThreadView => "thread_view",
            Self::ConversationView => "conversation_view",
            Self::Panel => "panel",
            Self::CommandPalette => "command_palette",
        })
    }
}

impl FromStr for KeybindContext {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Self::Global),
            "composer" => Ok(Self::Composer),
            "message_list" => Ok(Self::MessageList),
            "thread_view" => Ok(Self::ThreadView),
            "conversation_view" => Ok(Self::ConversationView),
            "panel" => Ok(Self::Panel),
            "command_palette" => Ok(Self::CommandPalette),
            _ => Err(format!("invalid keybind context `{s}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindSubject {
    User(UserId),
    Workspace(WorkspaceId),
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct NewKeybind {
    pub scope: KeybindScope,
    pub subject: KeybindSubject,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateKeybind {
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}
