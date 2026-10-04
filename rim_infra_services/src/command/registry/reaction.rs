use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        // P4-A5 naming decision (Tony): backend mutation is bidirectional,
        // so the agent tool is `chat.toggle_reaction` rather than
        // `chat.add_reaction`. The nl_description spells out the toggle
        // semantics so the LLM doesn't assume "add" when the user might
        // have already reacted.
        CommandDefinitionDto {
            id: "chat.toggle_reaction".to_string(),
            title: "Toggle Reaction".to_string(),
            description: "Add or remove an emoji reaction on a message.".to_string(),
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
                    description: "Identifier of the message to react to.".to_string(),
                    argument_type: CommandArgumentTypeDto::MessageId,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "emoji".to_string(),
                    title: "Emoji".to_string(),
                    description: "Unicode emoji or shortcode for the reaction.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
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
                "Add or remove an emoji reaction on a message. \
                 If the user has already reacted with this emoji, calling this removes the reaction."
                    .to_string(),
            ),
            intent_samples: vec![
                "react to that with thumbs up".to_string(),
                "add a 🎉 to jane's message".to_string(),
                "react :+1: to the announcement".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Toggle,
            rate_limit_per_minute: None,
            feature: None,
        },
    ]
}
