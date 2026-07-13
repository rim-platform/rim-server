use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

/// C1b (spec §10.1): registers `oncall.who`/`oncall.take_shift`. `oncall.who` mirrors the
/// `whoIsOnCall` GraphQL query; `oncall.take_shift` creates a short self-service override via
/// the same `create_override` method `createOnCallOverride` calls — the caller supplies the
/// window explicitly (no invented default duration), scoped to the acting user.
pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        CommandDefinitionDto {
            id: "oncall.who".to_string(),
            title: "Who Is On Call".to_string(),
            description: "Show who is currently on call for a schedule.".to_string(),
            category: "On-call".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![CommandArgumentDto {
                name: "schedule_id".to_string(),
                title: "Schedule".to_string(),
                description: "Identifier of the on-call schedule to query.".to_string(),
                argument_type: CommandArgumentTypeDto::String,
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
            typed_verb: TypedVerb::Read,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "oncall.take_shift".to_string(),
            title: "Take Shift".to_string(),
            description: "Create a self-service on-call override for the given window.".to_string(),
            category: "On-call".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![
                CommandArgumentDto {
                    name: "schedule_id".to_string(),
                    title: "Schedule".to_string(),
                    description: "Identifier of the on-call schedule to override.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "start_at".to_string(),
                    title: "Start".to_string(),
                    description: "RFC 3339 timestamp the override begins.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
                    required: true,
                    default_value: None,
                    allowed_values: None,
                },
                CommandArgumentDto {
                    name: "end_at".to_string(),
                    title: "End".to_string(),
                    description: "RFC 3339 timestamp the override ends.".to_string(),
                    argument_type: CommandArgumentTypeDto::String,
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
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Write,
            rate_limit_per_minute: None,
        },
    ]
}
