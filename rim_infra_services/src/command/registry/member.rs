use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    // AI-callable member listing, a Safe read that requires `chat.read`.
    // The executor is in `executor/member.rs`.
    vec![CommandDefinitionDto {
        id: "chat.list_members".to_string(),
        title: "List Members".to_string(),
        description: "List all members of a workspace.".to_string(),
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
            "List all members of a workspace with their usernames and roles.".to_string(),
        ),
        intent_samples: vec![
            "who's in this workspace".to_string(),
            "list everyone in the team".to_string(),
            "show all members".to_string(),
            "who can I message here".to_string(),
        ],
        ai_callable: true,
        sensitivity: CommandSensitivityDto::Safe,
        typed_verb: TypedVerb::Read,
        rate_limit_per_minute: None,
    }]
}
