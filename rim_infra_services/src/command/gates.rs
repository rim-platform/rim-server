use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::dispatch::Dispatch;

impl Dispatch {
    /// Returns the de-duplicated union of `required_capabilities` declared
    /// by every command the dispatch reaches (the single command, or every
    /// pipe stage).
    pub(super) fn required_capabilities(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let definitions: Vec<&CommandDefinitionDto> = match self {
            Self::Single(resolved) => resolved.iter().map(|r| &r.definition).collect(),
            Self::Pipe(stages) => stages
                .iter()
                .filter_map(|(_, r)| r.as_ref().map(|r| &r.definition))
                .collect(),
        };
        for def in definitions {
            for cap in &def.required_capabilities {
                if seen.insert(cap) {
                    out.push(cap.clone());
                }
            }
        }
        out
    }

    /// Returns `true` if dispatching for `initiator` requires a `ConfirmationProof`. A pipe
    /// is judged stage by stage up to its first unknown stage.
    pub(super) fn requires_confirmation(&self, initiator: CommandInitiator) -> bool {
        match self {
            Self::Single(resolved) => resolved
                .as_ref()
                .is_some_and(|r| r.definition.requires_confirmation_for_initiator(initiator)),
            Self::Pipe(stages) => {
                for (_, resolved) in stages {
                    let Some(resolved) = resolved else {
                        return false;
                    };
                    if resolved
                        .definition
                        .requires_confirmation_for_initiator(initiator)
                    {
                        return true;
                    }
                }
                false
            }
        }
    }
}

impl DefaultCommandService {
    /// Timeout gate: `Err(TimedOut { until })` when `definition` is Mutating (or
    /// stricter) and the actor is timed out in the context's workspace. Without a workspace
    /// context there is nothing to check here; the services behind the command still apply
    /// their own write gates.
    pub(super) async fn ensure_command_not_timed_out(
        &self,
        definition: &CommandDefinitionDto,
        actor: &Actor,
        context: Option<&CommandContextDto>,
    ) -> ServiceResult<()> {
        if definition.sensitivity < CommandSensitivityDto::Mutating {
            return Ok(());
        }
        let Some(server_id) = context.and_then(|c| c.workspace_id) else {
            return Ok(());
        };
        match self
            .services
            .moderation_service
            .active_timeout(server_id, actor.user_id)
            .await?
        {
            Some(until) => Err(ServiceError::TimedOut { until }),
            None => Ok(()),
        }
    }
}
