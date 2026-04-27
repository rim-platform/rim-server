use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![CommandDefinitionDto {
        id: "chat.create_group".to_string(),
        title: "Create Group".to_string(),
        description: "Create a new group conversation.".to_string(),
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
                name: "member_ids".to_string(),
                title: "Members".to_string(),
                description: "Member ids to include in the group.".to_string(),
                argument_type: CommandArgumentTypeDto::String,
                required: true,
                default_value: None,
                allowed_values: None,
            },
            CommandArgumentDto {
                name: "title".to_string(),
                title: "Title".to_string(),
                description: "Optional group title.".to_string(),
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
        nl_description: None,
        intent_samples: vec![],
        ai_callable: false,
        sensitivity: CommandSensitivityDto::Safe,
        typed_verb: TypedVerb::Write,
        rate_limit_per_minute: None,
    }]
}
