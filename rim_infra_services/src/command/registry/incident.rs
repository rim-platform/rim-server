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
/// C2 (`docs/specs/track-c2-agent-operator.md` §1): both flip to `ai_callable: true` under the
/// existing agentic confirmation gate (`requires_confirmation_for_initiator`) — Mutating
/// sensitivity means an agentic initiator always needs a confirmation proof; human initiators
/// (Palette/Keybind/ContextMenu/Cli) stay one-shot, unchanged from C1.
pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        CommandDefinitionDto {
            id: "incident.ack".to_string(),
            title: "Acknowledge Incident".to_string(),
            // C2 §1d: strengthened to state the consequence explicitly — this is the FE-facing
            // agent-confirmation warning copy (see §4: the confirmation modal sources its
            // consequence line from `Command.description`, not a challenge-carried message).
            description: "Acknowledge an incident. This cancels the escalation chain and \
                          stops paging on-call for this incident."
                .to_string(),
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
            nl_description: Some(
                "Acknowledge an incident on behalf of the user. Acknowledging cancels \
                 the escalation chain and stops paging on-call for this incident, so \
                 only call it when the user clearly wants to take ownership and stop \
                 the paging."
                    .to_string(),
            ),
            intent_samples: vec![
                "ack the checkout incident".to_string(),
                "acknowledge this incident".to_string(),
                "I've got this incident, stop paging".to_string(),
                "mark the incident as acknowledged".to_string(),
            ],
            ai_callable: true,
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
            nl_description: Some(
                "Immediately fire the incident's current escalation step instead of \
                 waiting for the timer, paging the next responders in the chain. Use \
                 when the user asks to escalate an incident now."
                    .to_string(),
            ),
            intent_samples: vec![
                "escalate this now".to_string(),
                "escalate the checkout incident".to_string(),
                "page the next level for this incident".to_string(),
                "skip the wait and escalate".to_string(),
            ],
            ai_callable: true,
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Execute,
            rate_limit_per_minute: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C2 §6.1: both incident lifecycle commands flip to `ai_callable: true` with populated
    /// AI metadata, `required_capabilities` stays empty (§2 ruling), and the sensitivity /
    /// dangerous / typed_verb triple is unchanged from C1 (pins "no downgrade").
    #[test]
    fn incident_ack_and_escalate_are_ai_callable_with_c2_metadata_and_unchanged_sensitivity() {
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
            // No downgrade: both remain Mutating/Execute/dangerous=false, exactly as C1 shipped.
            assert_eq!(def.sensitivity, CommandSensitivityDto::Mutating);
            assert_eq!(def.typed_verb, TypedVerb::Execute);
            assert!(!def.dangerous);
        }
    }

    /// C2 §1d: `incident.ack`'s `description` is rewritten to state the paging-stop consequence
    /// explicitly — this is the FE-facing agent-confirmation warning copy (§4).
    #[test]
    fn incident_ack_description_states_the_paging_consequence() {
        let defs = commands();
        let ack = defs
            .iter()
            .find(|d| d.id == "incident.ack")
            .expect("incident.ack must be registered");
        assert!(ack.description.contains("cancels the escalation chain"));
        assert!(ack.description.contains("stops paging on-call"));
    }
}
