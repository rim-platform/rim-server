use rim_core::command::model::UserCommandId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandSourceKindDto {
    BuiltIn,
    Plugin,
    Integration,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandExecutionTargetDto {
    Backend,
    DesktopLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandSurfaceDto {
    Palette,
    ContextMenu,
    KeybindPicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandArgumentTypeDto {
    String,
    Integer,
    Float,
    Boolean,
    Enum,
    UserId,
    ConversationId,
    MessageId,
    ThreadId,
    WorkspaceId,
    SelectionText,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandArgumentDto {
    pub name: String,
    pub title: String,
    pub description: String,
    pub argument_type: CommandArgumentTypeDto,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
    pub allowed_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandContextDto {
    pub workspace_id: Option<rim_core::server::model::ServerId>,
    pub conversation_id: Option<rim_core::conversation::model::ConversationId>,
    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub selected_text: Option<String>,
    pub selected_message_id: Option<rim_core::message::model::MessageId>,
    pub selected_message_ids: Vec<rim_core::message::model::MessageId>,
    pub focused_surface: Option<String>,
    pub active_panel: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandDefinitionDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: CommandSourceKindDto,
    pub execution_target: CommandExecutionTargetDto,
    pub arguments: Vec<CommandArgumentDto>,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub visible_in_palette: bool,
    pub visible_in_context_menus: bool,
    pub keybind_capable: bool,
    pub dangerous: bool,
    pub user_command_id: Option<UserCommandId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandResultStatusDto {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandResultDto {
    pub status: CommandResultStatusDto,
    pub message: String,
    pub payload: Option<serde_json::Value>,
    pub follow_up_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserCommandStepDto {
    pub command_id: String,
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserCommandDto {
    pub id: UserCommandId,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
}

impl UserCommandDto {
    pub fn as_command_definition(&self) -> CommandDefinitionDto {
        self.as_command_definition_with_metadata(
            CommandExecutionTargetDto::Backend,
            vec![],
            vec![],
            true,
            false,
        )
    }

    pub fn as_command_definition_with_metadata(
        &self,
        execution_target: CommandExecutionTargetDto,
        required_context: Vec<String>,
        required_capabilities: Vec<String>,
        keybind_capable: bool,
        dangerous: bool,
    ) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: self.command_key.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            category: "User".to_string(),
            source_kind: CommandSourceKindDto::User,
            execution_target,
            arguments: vec![],
            required_context,
            required_capabilities,
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable,
            dangerous,
            user_command_id: Some(self.id),
        }
    }
}
