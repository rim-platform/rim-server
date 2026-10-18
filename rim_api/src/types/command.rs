use async_graphql::{Enum, InputObject, Result, SimpleObject};

// ── GraphQL output enums (stable wire contract) ──────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandSourceKind {
    BuiltIn,
    Plugin,
    Integration,
    User,
}

impl From<rim_services::command::dto::CommandSourceKindDto> for CommandSourceKind {
    fn from(value: rim_services::command::dto::CommandSourceKindDto) -> Self {
        use rim_services::command::dto::CommandSourceKindDto as D;
        match value {
            D::BuiltIn => Self::BuiltIn,
            D::Plugin => Self::Plugin,
            D::Integration => Self::Integration,
            D::User => Self::User,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandExecutionTarget {
    Backend,
    DesktopLocal,
}

impl From<rim_services::command::dto::CommandExecutionTargetDto> for CommandExecutionTarget {
    fn from(value: rim_services::command::dto::CommandExecutionTargetDto) -> Self {
        use rim_services::command::dto::CommandExecutionTargetDto as D;
        match value {
            D::Backend => Self::Backend,
            D::DesktopLocal => Self::DesktopLocal,
        }
    }
}

/// How much a command can affect. It decides whether a call needs a confirmation proof and is
/// recorded in the audit log; clients use it to cap an agent's mutating calls.
// Mirrors `CommandSensitivityDto`; the exhaustive `From` impls below fail to compile when a
// variant is added on one side only.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandSensitivity {
    /// Read-only, no user-visible side effects.
    Safe,
    /// Tracked in audit but low blast radius.
    Sensitive,
    /// Mutates durable state; counts against agent mutating-call cap.
    Mutating,
    /// High-blast-radius admin/destructive; always requires confirmation.
    Restricted,
}

impl From<rim_services::command::sensitivity::CommandSensitivityDto> for CommandSensitivity {
    fn from(value: rim_services::command::sensitivity::CommandSensitivityDto) -> Self {
        use rim_services::command::sensitivity::CommandSensitivityDto as D;
        match value {
            D::Safe => Self::Safe,
            D::Sensitive => Self::Sensitive,
            D::Mutating => Self::Mutating,
            D::Restricted => Self::Restricted,
        }
    }
}

impl From<CommandSensitivity> for rim_services::command::sensitivity::CommandSensitivityDto {
    fn from(value: CommandSensitivity) -> Self {
        match value {
            CommandSensitivity::Safe => Self::Safe,
            CommandSensitivity::Sensitive => Self::Sensitive,
            CommandSensitivity::Mutating => Self::Mutating,
            CommandSensitivity::Restricted => Self::Restricted,
        }
    }
}

/// Where a command execution came from. The server uses it for the audit log and the
/// confirmation policy: `AGENT` and `PLUGIN` executions of mutating commands need a
/// confirmation proof.
// Mirrors `rim_services::command::initiator::CommandInitiator`; the exhaustive `From` impls
// below fail to compile when a variant is added on one side only.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandInitiator {
    /// User typed/clicked in the command palette.
    Palette,
    /// AI agent issued the command via the dispatcher.
    Agent,
    /// Inside a macro / user command expansion.
    Macro,
    /// Keyboard shortcut bound to a command.
    Keybind,
    /// Context menu (right-click) on a chat element.
    ContextMenu,
    /// Plugin's own runtime invoked the command.
    Plugin,
    /// Out-of-process CLI client (rim-cli).
    Cli,
}

impl From<rim_services::command::initiator::CommandInitiator> for CommandInitiator {
    fn from(value: rim_services::command::initiator::CommandInitiator) -> Self {
        use rim_services::command::initiator::CommandInitiator as D;
        match value {
            D::Palette => Self::Palette,
            D::Agent => Self::Agent,
            D::Macro => Self::Macro,
            D::Keybind => Self::Keybind,
            D::ContextMenu => Self::ContextMenu,
            D::Plugin => Self::Plugin,
            D::Cli => Self::Cli,
        }
    }
}

impl From<CommandInitiator> for rim_services::command::initiator::CommandInitiator {
    fn from(value: CommandInitiator) -> Self {
        use rim_services::command::initiator::CommandInitiator as D;
        match value {
            CommandInitiator::Palette => D::Palette,
            CommandInitiator::Agent => D::Agent,
            CommandInitiator::Macro => D::Macro,
            CommandInitiator::Keybind => D::Keybind,
            CommandInitiator::ContextMenu => D::ContextMenu,
            CommandInitiator::Plugin => D::Plugin,
            CommandInitiator::Cli => D::Cli,
        }
    }
}

/// The type of a command argument. Clients map each value to a form control and to a JSON
/// schema fragment when they describe the command to an AI agent.
// Mirrors `CommandArgumentTypeDto`; the exhaustive `From` impl below fails to compile when a
// variant is added on the DTO only.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandArgumentType {
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

impl From<rim_services::command::dto::CommandArgumentTypeDto> for CommandArgumentType {
    fn from(value: rim_services::command::dto::CommandArgumentTypeDto) -> Self {
        use rim_services::command::dto::CommandArgumentTypeDto as D;
        match value {
            D::String => Self::String,
            D::Integer => Self::Integer,
            D::Float => Self::Float,
            D::Boolean => Self::Boolean,
            D::Enum => Self::Enum,
            D::UserId => Self::UserId,
            D::ConversationId => Self::ConversationId,
            D::MessageId => Self::MessageId,
            D::ThreadId => Self::ThreadId,
            D::WorkspaceId => Self::WorkspaceId,
            D::SelectionText => Self::SelectionText,
        }
    }
}

/// One argument a command accepts. `defaultValue` is JSON because arguments may be strings,
/// numbers, booleans or objects; `allowedValues` lists the choices of an `ENUM` argument.
#[derive(SimpleObject, Clone, Debug)]
pub struct CommandArgument {
    pub name: String,
    pub title: String,
    pub description: String,
    pub argument_type: CommandArgumentType,
    pub required: bool,
    pub default_value: Option<async_graphql::Json<serde_json::Value>>,
    pub allowed_values: Option<Vec<String>>,
}

impl From<rim_services::command::dto::CommandArgumentDto> for CommandArgument {
    fn from(value: rim_services::command::dto::CommandArgumentDto) -> Self {
        Self {
            name: value.name,
            title: value.title,
            description: value.description,
            argument_type: value.argument_type.into(),
            required: value.required,
            default_value: value.default_value.map(async_graphql::Json),
            allowed_values: value.allowed_values,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: CommandSourceKind,
    pub execution_target: CommandExecutionTarget,
    /// The arguments the command accepts, in order. Clients build the palette form and the
    /// AI tool schema from them.
    pub arguments: Vec<CommandArgument>,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub visible_in_palette: bool,
    pub visible_in_context_menus: bool,
    pub keybind_capable: bool,
    pub dangerous: bool,
    /// AI-optimised natural-language description; falls back to `description`
    /// when absent. Sanitized at registration time, so the frontend can trust
    /// this field even when rendering inside a prompt template.
    pub nl_description: Option<String>,
    /// Example phrases a user might speak/type to invoke this command.
    /// Individually sanitized at registration; bounded in count and length.
    pub intent_samples: Vec<String>,
    /// `true` when an AI agent may call the command. Only these commands appear on the AI
    /// surface and in `commandDiscover`. Always `false` while the instance has the AI command
    /// surface off.
    pub ai_callable: bool,
    /// Classification tier driving audit verbosity, mutating-call caps, and
    /// the UX confirm-modal placement.
    pub sensitivity: CommandSensitivity,
}

impl From<rim_services::command::dto::CommandDefinitionDto> for Command {
    fn from(value: rim_services::command::dto::CommandDefinitionDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            category: value.category,
            source_kind: value.source_kind.into(),
            execution_target: value.execution_target.into(),
            arguments: value.arguments.into_iter().map(Into::into).collect(),
            required_context: value.required_context,
            required_capabilities: value.required_capabilities,
            visible_in_palette: value.visible_in_palette,
            visible_in_context_menus: value.visible_in_context_menus,
            keybind_capable: value.keybind_capable,
            dangerous: value.dangerous,
            nl_description: value.nl_description,
            intent_samples: value.intent_samples,
            ai_callable: value.ai_callable,
            sensitivity: value.sensitivity.into(),
        }
    }
}

/// One step of a user command: the command it runs and the arguments it passes.
#[derive(SimpleObject, Clone, Debug)]
pub struct UserCommandStep {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

/// A user command as its owner saved it, in the shape `updateUserCommand` takes.
#[derive(SimpleObject, Clone, Debug)]
pub struct UserCommand {
    /// The command id, `user.<key>`, the same as the listed `Command.id`.
    pub id: String,
    pub title: String,
    pub description: String,
    /// The steps in the order they run.
    pub steps: Vec<UserCommandStep>,
    /// `false` while the command is disabled; a disabled command is not listed by `commands`.
    pub is_enabled: bool,
}

impl From<rim_services::command::dto::UserCommandDto> for UserCommand {
    fn from(value: rim_services::command::dto::UserCommandDto) -> Self {
        Self {
            id: value.command_key,
            title: value.title,
            description: value.description,
            steps: value
                .steps
                .into_iter()
                .map(|step| UserCommandStep {
                    command_id: step.command_id,
                    args: step.args.map(async_graphql::Json),
                })
                .collect(),
            is_enabled: value.is_enabled,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandSurface {
    Palette,
    ContextMenu,
    KeybindPicker,
    Ai,
}

impl From<CommandSurface> for rim_services::command::dto::CommandSurfaceDto {
    fn from(value: CommandSurface) -> Self {
        match value {
            CommandSurface::Palette => Self::Palette,
            CommandSurface::ContextMenu => Self::ContextMenu,
            CommandSurface::KeybindPicker => Self::KeybindPicker,
            CommandSurface::Ai => Self::Ai,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct CommandContextInput {
    pub workspace_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
    pub thread_id: Option<async_graphql::ID>,
    pub selected_text: Option<String>,
    pub selected_message_id: Option<async_graphql::ID>,
    pub selected_message_ids: Option<Vec<async_graphql::ID>>,
    pub focused_surface: Option<String>,
    pub active_panel: Option<String>,
}

impl TryFrom<CommandContextInput> for rim_services::command::dto::CommandContextDto {
    type Error = async_graphql::Error;

    fn try_from(value: CommandContextInput) -> Result<Self, Self::Error> {
        Ok(Self {
            workspace_id: value
                .workspace_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>)
                .transpose()?,
            conversation_id: value
                .conversation_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::conversation::model::ConversationId>)
                .transpose()?,
            thread_id: value
                .thread_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>)
                .transpose()?,
            selected_text: value.selected_text,
            selected_message_id: value
                .selected_message_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>)
                .transpose()?,
            selected_message_ids: value
                .selected_message_ids
                .unwrap_or_default()
                .into_iter()
                .map(|id| crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(&id))
                .collect::<Result<Vec<_>, _>>()?,
            focused_surface: value.focused_surface,
            active_panel: value.active_panel,
        })
    }
}
