use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![CommandDefinitionDto {
        id: "admin.invite_member".to_string(),
        title: "Invite Member".to_string(),
        description: "Invite a new member to the workspace.".to_string(),
        category: "Admin".to_string(),
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
                name: "user_id".to_string(),
                title: "User".to_string(),
                description: "User identifier to invite.".to_string(),
                argument_type: CommandArgumentTypeDto::String,
                required: true,
                default_value: None,
                allowed_values: None,
            },
        ],
        required_context: vec![],
        required_capabilities: vec!["admin.manage_members".to_string()],
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
