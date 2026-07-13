use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

/// C1b (spec §10.1): registers `incident.ack`/`incident.escalate` so the same dispatcher used
/// by keybinds/palette/pipelines can trigger the two incident-lifecycle actions that also have
/// GraphQL mutations (`acknowledgeIncident`, C1c) — each executor calls the identical
/// `IncidentService` method its mutation calls (unified dispatch, no duplicated logic).
/// `ai_callable: false` for both per iron-man's ruling — C2 flips this later, it does not
/// re-architect the registration.
pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        CommandDefinitionDto {
            id: "incident.ack".to_string(),
            title: "Acknowledge Incident".to_string(),
            description: "Acknowledge an incident, cancelling its escalation chain.".to_string(),
            category: "Incident".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![CommandArgumentDto {
                name: "incident_id".to_string(),
                title: "Incident".to_string(),
                description: "Identifier of the incident to acknowledge.".to_string(),
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
            // §10.1: the honest sensitivity for a state transition is Mutating — do NOT
            // downgrade to Safe just to dodge a confirmation prompt that doesn't exist for
            // non-agentic initiators anyway (Safe would mislabel a mutation for audit).
            dangerous: false,
            user_command_id: None,
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Execute,
            rate_limit_per_minute: None,
        },
        CommandDefinitionDto {
            id: "incident.escalate".to_string(),
            title: "Escalate Incident Now".to_string(),
            description: "Fire the incident's current escalation step immediately.".to_string(),
            category: "Incident".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![CommandArgumentDto {
                name: "incident_id".to_string(),
                title: "Incident".to_string(),
                description: "Identifier of the incident to escalate.".to_string(),
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
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Execute,
            rate_limit_per_minute: None,
        },
    ]
}
