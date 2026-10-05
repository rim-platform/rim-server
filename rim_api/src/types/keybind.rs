use async_graphql::{Enum, SimpleObject};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindSource {
    Default,
    User,
    Workspace,
}

impl From<rim_services::keybind::dto::KeybindSourceDto> for KeybindSource {
    fn from(value: rim_services::keybind::dto::KeybindSourceDto) -> Self {
        use rim_services::keybind::dto::KeybindSourceDto as D;
        match value {
            D::Default => Self::Default,
            D::User => Self::User,
            D::Workspace => Self::Workspace,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Keybind {
    pub id: async_graphql::ID,
    pub source: KeybindSource,
    pub scope: KeybindScope,
    pub workspace_id: Option<async_graphql::ID>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
    pub is_enabled: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct DefaultKeybind {
    pub source: KeybindSource,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::keybind::dto::DefaultKeybindDto> for DefaultKeybind {
    fn from(value: rim_services::keybind::dto::DefaultKeybindDto) -> Self {
        Self {
            source: value.source.into(),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: value.context.into(),
            preset_args: value.preset_args.map(async_graphql::Json),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct KeybindLayers {
    pub default_keybind: Option<DefaultKeybind>,
    pub user_keybind: Option<Keybind>,
    pub workspace_keybind: Option<Keybind>,
    pub resolved_persisted_keybind: Option<Keybind>,
}

impl From<rim_services::keybind::dto::KeybindLayersDto> for KeybindLayers {
    fn from(value: rim_services::keybind::dto::KeybindLayersDto) -> Self {
        Self {
            default_keybind: value.default_keybind.map(Into::into),
            user_keybind: value.user_keybind.map(Into::into),
            workspace_keybind: value.workspace_keybind.map(Into::into),
            resolved_persisted_keybind: value.resolved_persisted_keybind.map(Into::into),
        }
    }
}

impl From<rim_services::keybind::dto::KeybindDto> for Keybind {
    fn from(value: rim_services::keybind::dto::KeybindDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            source: value.source.into(),
            scope: value.scope.into(),
            workspace_id: value
                .workspace_id
                .map(|id| async_graphql::ID(id.to_string())),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: value.context.into(),
            preset_args: value.preset_args.map(async_graphql::Json),
            is_enabled: value.is_enabled,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindScope {
    User,
    Workspace,
}

impl From<KeybindScope> for rim_services::keybind::dto::KeybindScopeDto {
    fn from(value: KeybindScope) -> Self {
        match value {
            KeybindScope::User => Self::User,
            KeybindScope::Workspace => Self::Workspace,
        }
    }
}

impl From<rim_services::keybind::dto::KeybindScopeDto> for KeybindScope {
    fn from(value: rim_services::keybind::dto::KeybindScopeDto) -> Self {
        use rim_services::keybind::dto::KeybindScopeDto as D;
        match value {
            D::User => Self::User,
            D::Workspace => Self::Workspace,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindContext {
    Global,
    Composer,
    MessageList,
    ThreadView,
    ConversationView,
    Panel,
    CommandPalette,
}

impl From<KeybindContext> for rim_services::keybind::dto::KeybindContextDto {
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

impl From<rim_services::keybind::dto::KeybindContextDto> for KeybindContext {
    fn from(value: rim_services::keybind::dto::KeybindContextDto) -> Self {
        use rim_services::keybind::dto::KeybindContextDto as D;
        match value {
            D::Global => Self::Global,
            D::Composer => Self::Composer,
            D::MessageList => Self::MessageList,
            D::ThreadView => Self::ThreadView,
            D::ConversationView => Self::ConversationView,
            D::Panel => Self::Panel,
            D::CommandPalette => Self::CommandPalette,
        }
    }
}
