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
/// C2 (`docs/specs/track-c2-agent-operator.md` §1): both flip to `ai_callable: true`.
/// `oncall.who` is Safe — one-shot for every initiator, including Agent. `oncall.take_shift` is
/// Mutating, so an agentic initiator needs a confirmation proof via the existing
/// `requires_confirmation_for_initiator` gate; human initiators are unaffected.
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
            nl_description: Some(
                "Look up who is currently on call for an on-call schedule, including \
                 when their shift ends and who is next. Use when the user asks who is \
                 on call or who to page."
                    .to_string(),
            ),
            intent_samples: vec![
                "who's on call for payments".to_string(),
                "who is on call right now".to_string(),
                "who do I page for this".to_string(),
                "when does the current on-call shift end".to_string(),
            ],
            ai_callable: true,
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
            nl_description: Some(
                "Create an on-call override that puts the requesting user on call for \
                 an explicit time window. Requires RFC 3339 start and end timestamps, \
                 and the requesting user must have on-call schedule management rights \
                 (server owner or moderator) — a plain member's request will be denied. \
                 Use when an authorized user asks to take, cover, or swap onto a shift."
                    .to_string(),
            ),
            intent_samples: vec![
                "put me on call until 6pm".to_string(),
                "I'll take the on-call shift tonight".to_string(),
                "cover the shift from 2pm to 8pm for me".to_string(),
                "take over on-call for the next four hours".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Write,
            rate_limit_per_minute: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C2 §6.1: both on-call commands flip to `ai_callable: true` with populated AI metadata,
    /// `required_capabilities` stays empty (§2 ruling), and sensitivity/dangerous/typed_verb are
    /// unchanged from C1 (pins "no downgrade").
    #[test]
    fn oncall_who_and_take_shift_are_ai_callable_with_c2_metadata_and_unchanged_sensitivity() {
        let defs = commands();
        assert_eq!(defs.len(), 2);

        for def in &defs {
            assert!(def.ai_callable, "{} must be ai_callable: true", def.id);
            assert!(
                def.nl_description.is_some(),
                "{} must have a non-empty nl_description",
                def.id
            );
            assert!(
                !def.nl_description
                    .as_ref()
                    .expect("checked Some above")
                    .is_empty(),
                "{} nl_description must not be empty",
                def.id
            );
            assert_eq!(
                def.intent_samples.len(),
                4,
                "{} must have exactly 4 intent samples",
                def.id
            );
            assert!(
                def.required_capabilities.is_empty(),
                "{} required_capabilities must stay empty (C2 §2 ruling)",
                def.id
            );
        }

        let who = defs
            .iter()
            .find(|d| d.id == "oncall.who")
            .expect("oncall.who must be registered");
        assert_eq!(who.sensitivity, CommandSensitivityDto::Safe);
        assert_eq!(who.typed_verb, TypedVerb::Read);
        assert!(!who.dangerous);

        let take_shift = defs
            .iter()
            .find(|d| d.id == "oncall.take_shift")
            .expect("oncall.take_shift must be registered");
        assert_eq!(take_shift.sensitivity, CommandSensitivityDto::Mutating);
        assert_eq!(take_shift.typed_verb, TypedVerb::Write);
        assert!(!take_shift.dangerous);
    }
}
