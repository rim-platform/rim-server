use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![CommandDefinitionDto {
        id: "chat.pin_message".to_string(),
        title: "Pin Message".to_string(),
        description: "Pin a message in a channel.".to_string(),
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
                description: "Channel/conversation containing the message.".to_string(),
                argument_type: CommandArgumentTypeDto::ConversationId,
                required: true,
                default_value: None,
                allowed_values: None,
            },
            CommandArgumentDto {
                name: "message_id".to_string(),
                title: "Message".to_string(),
                description: "Identifier of the message to pin.".to_string(),
                argument_type: CommandArgumentTypeDto::MessageId,
                required: true,
                default_value: None,
                allowed_values: None,
            },
        ],
        required_context: vec![],
        required_capabilities: vec!["chat.write".to_string()],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: false,
        dangerous: false,
        user_command_id: None,
        nl_description: Some(
            "Pin a message in a channel so it appears in the channel's pinned list.".to_string(),
        ),
        intent_samples: vec![
            "pin that message".to_string(),
            "pin the announcement".to_string(),
            "save this message to pinned".to_string(),
        ],
        ai_callable: true,
        sensitivity: CommandSensitivityDto::Mutating,
        typed_verb: TypedVerb::Write,
        rate_limit_per_minute: None,
        feature: None,
    }]
}
