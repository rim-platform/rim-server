use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        // `ai_callable: true` puts it on the surface filtered by
        // `CommandSurfaceDto::Ai`. `chat.read` is the only required cap and
        // is seeded in `builtin_capabilities()`.
        CommandDefinitionDto {
            id: "chat.list_channels".to_string(),
            title: "List Channels".to_string(),
            description: "List the channels you can see in the current workspace.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![CommandArgumentDto {
                name: "server_id".to_string(),
                title: "Workspace".to_string(),
                description: "Workspace/server identifier.".to_string(),
                argument_type: CommandArgumentTypeDto::WorkspaceId,
                required: true,
                default_value: None,
                allowed_values: None,
            }],
            required_context: vec![],
            required_capabilities: vec!["chat.read".to_string()],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: false,
            dangerous: false,
            user_command_id: None,
            nl_description: Some(
                "Lists the channels in the current workspace the user can see (public ones \
                 and private ones they belong to, archived excluded), returning name, ID, \
                 conversation type, and whether each is private, archived, and joined by the \
                 user. Use this when the user asks 'what channels are here?', wants to \
                 navigate to a channel by name, or before joining or inviting to one."
                    .to_string(),
            ),
            intent_samples: vec![
                "list channels".to_string(),
                "what channels do we have".to_string(),
                "show me the channels".to_string(),
                "which channels are in this workspace".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: TypedVerb::Read,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "chat.create_channel".to_string(),
            title: "Create Channel".to_string(),
            description: "Create a new channel.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![
                CommandArgumentDto {
                    name: "server_id".to_string(),
                    title: "Server".to_string(),
                    description: "Workspace/server identifier.".to_string(),
                    argument_type: CommandArgumentTypeDto::WorkspaceId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "name".to_string(),
                    title: "Name".to_string(),
                    description: "Channel name.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "description".to_string(),
                    title: "Description".to_string(),
                    description: "Optional channel topic / description.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: false,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "is_private".to_string(),
                    title: "Private".to_string(),
                    description: "Only invited members can see the channel. Cannot be changed \
                                  later. Defaults to false."
                        .to_string(),
                    argument_type: CommandArgumentTypeDto::Boolean,
                    required: false,
                    default_value: Some(serde_json::Value::Bool(false)),
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "member_ids".to_string(),
                    title: "Members".to_string(),
                    description: "Optional user ids to add as members right after creation."
                        .to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: false,
                    default_value: None,
                    allowed_values: None,
                },
            ],
            required_context: vec![],
            required_capabilities: vec!["chat.write".to_string()],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: true,
            dangerous: false,
            user_command_id: None,
            nl_description: Some(
                "Create a new channel in the workspace, public by default or private \
                 (members only, set is_private) with optional initial members. \
                 Use when the user asks to create a new channel or start a new topic."
                    .to_string(),
            ),
            intent_samples: vec![
                "make a channel for the migration project".to_string(),
                "create #incident-2026-04-24".to_string(),
                "start a new channel called design-reviews".to_string(),
                "create a private channel for the hiring committee".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Write,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "chat.mark_as_read".to_string(),
            title: "Mark as Read".to_string(),
            description: "Mark the current conversation as read.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![
                CommandArgumentDto {
                    name: "server_id".to_string(),
                    title: "Server".to_string(),
                    description: "Workspace/server identifier.".to_string(),
                    argument_type: CommandArgumentTypeDto::WorkspaceId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "conversation_id".to_string(),
                    title: "Conversation".to_string(),
                    description:
                        "Conversation to mark as read; defaults to the focused conversation."
                            .to_string(),
                    argument_type: CommandArgumentTypeDto::ConversationId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
            ],
            required_context: vec![],
            required_capabilities: vec![],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: true,
            dangerous: false,
            user_command_id: None,
            nl_description: Some(
                "Mark one conversation as read for the current user. \
                 Clears its unread count."
                    .to_string(),
            ),
            intent_samples: vec![
                "mark #general as read".to_string(),
                "mark this channel read".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Write,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "chat.mark_all_as_read".to_string(),
            title: "Mark All as Read".to_string(),
            description: "Mark all conversations as read.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![CommandArgumentDto {
                name: "server_id".to_string(),
                title: "Server".to_string(),
                description: "Workspace/server identifier.".to_string(),
                argument_type: CommandArgumentTypeDto::WorkspaceId,
                required: true,
                default_value: None,
                allowed_values: None,
            }],
            required_context: vec![],
            required_capabilities: vec![],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: true,
            dangerous: false,
            user_command_id: None,
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: TypedVerb::Write,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "chat.search_messages".to_string(),
            title: "Search Messages".to_string(),
            description: "Search workspace messages by text content.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![
                CommandArgumentDto {
                    name: "server_id".to_string(),
                    title: "Workspace".to_string(),
                    description: "Workspace/server identifier.".to_string(),
                    argument_type: CommandArgumentTypeDto::WorkspaceId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "query".to_string(),
                    title: "Query".to_string(),
                    description: "Free-text search query.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "conversation_id".to_string(),
                    title: "Conversation".to_string(),
                    description: "Optional channel/conversation to scope the search to."
                        .to_string(),
                    argument_type: CommandArgumentTypeDto::ConversationId,
                    required: false,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "limit".to_string(),
                    title: "Limit".to_string(),
                    description: "Max results to return (default 10, max 50).".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: false,
                    default_value: None,
                    allowed_values: None,
                },
            ],
            required_context: vec![],
            required_capabilities: vec!["chat.read".to_string()],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: false,
            dangerous: false,
            user_command_id: None,
            nl_description: Some(
                "Search workspace messages by text content. \
                 Use to find specific past messages or topics."
                    .to_string(),
            ),
            intent_samples: vec![
                "find messages about the launch".to_string(),
                "search for 'auth migration'".to_string(),
                "did anyone mention the bug from Tuesday".to_string(),
                "find what jane said about deploys".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: TypedVerb::Read,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "chat.get_recent_messages".to_string(),
            title: "Get Recent Messages".to_string(),
            description: "Read the most recent messages in a channel.".to_string(),
            category: "Chat".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![
                CommandArgumentDto {
                    name: "server_id".to_string(),
                    title: "Workspace".to_string(),
                    description: "Workspace/server identifier.".to_string(),
                    argument_type: CommandArgumentTypeDto::WorkspaceId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "conversation_id".to_string(),
                    title: "Conversation".to_string(),
                    description:
                        "Channel/conversation to read from; defaults to the focused conversation."
                            .to_string(),
                    argument_type: CommandArgumentTypeDto::ConversationId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "limit".to_string(),
                    title: "Limit".to_string(),
                    description: "Max messages to return (default 30, max 100).".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: false,
                    default_value: None,
                    allowed_values: None,
                },
            ],
            required_context: vec![],
            required_capabilities: vec!["chat.read".to_string()],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: false,
            dangerous: false,
            user_command_id: None,
            nl_description: Some(
                "Read the most recent messages in a channel. \
                 Use for summaries, catching up, or quoting back what someone said."
                    .to_string(),
            ),
            intent_samples: vec![
                "summarize what's been said in #engineering today".to_string(),
                "what did the team discuss this morning".to_string(),
                "catch me up on the design channel".to_string(),
                "read the last 50 messages in here".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: TypedVerb::Read,
            rate_limit_per_minute: None,
        },
    ]
}
