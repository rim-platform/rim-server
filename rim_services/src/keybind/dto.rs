use rim_core::keybind::model::{KeybindContext, KeybindId, KeybindScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindSourceDto {
    Default,
    User,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindScopeDto {
    User,
    Workspace,
}

impl From<KeybindScope> for KeybindScopeDto {
    fn from(value: KeybindScope) -> Self {
        match value {
            KeybindScope::User => Self::User,
            KeybindScope::Workspace => Self::Workspace,
        }
    }
}

impl From<KeybindScopeDto> for KeybindScope {
    fn from(value: KeybindScopeDto) -> Self {
        match value {
            KeybindScopeDto::User => Self::User,
            KeybindScopeDto::Workspace => Self::Workspace,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindContextDto {
    Global,
    Composer,
    MessageList,
    ThreadView,
    ConversationView,
    Panel,
    CommandPalette,
}

impl From<KeybindContext> for KeybindContextDto {
    fn from(value: KeybindContext) -> Self {
        match value {
            KeybindContext::Global => Self::Global,
            KeybindContext::Composer => Self::Composer,
            KeybindContext::MessageList => Self::MessageList,
            KeybindContext::ThreadView => Self::ThreadView,
            KeybindContext::ConversationView => Self::ConversationView,
            KeybindContext::Panel => Self::Panel,
            KeybindContext::CommandPalette => Self::CommandPalette,
        }
    }
}

impl From<KeybindContextDto> for KeybindContext {
    fn from(value: KeybindContextDto) -> Self {
        match value {
            KeybindContextDto::Global => KeybindContext::Global,
            KeybindContextDto::Composer => KeybindContext::Composer,
            KeybindContextDto::MessageList => KeybindContext::MessageList,
            KeybindContextDto::ThreadView => KeybindContext::ThreadView,
            KeybindContextDto::ConversationView => KeybindContext::ConversationView,
            KeybindContextDto::Panel => KeybindContext::Panel,
            KeybindContextDto::CommandPalette => KeybindContext::CommandPalette,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeybindDto {
    pub id: KeybindId,
    pub scope: KeybindScopeDto,
    pub source: KeybindSourceDto,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefaultKeybindDto {
    pub source: KeybindSourceDto,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeybindLayersDto {
    pub default_keybind: Option<DefaultKeybindDto>,
    pub user_keybind: Option<KeybindDto>,
    pub workspace_keybind: Option<KeybindDto>,
    pub resolved_persisted_keybind: Option<KeybindDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindRuntimeSourceDto {
    DeviceLocal,
    Workspace,
    User,
    Default,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceLocalKeybindDto {
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveKeybindDto {
    pub source: KeybindRuntimeSourceDto,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeKeybindLayersDto {
    pub device_local_keybind: Option<DeviceLocalKeybindDto>,
    pub backend_layers: KeybindLayersDto,
    pub resolved_effective_keybind: Option<EffectiveKeybindDto>,
}
