use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto};
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::pipe_parser;

impl DefaultCommandService {
    /// Returns the de-duplicated union of `required_capabilities` declared
    /// by every command the dispatch path will reach (top-level builtin or
    /// user-command, plus all pipe stages).
    pub(super) async fn dispatch_required_capabilities(
        &self,
        cmd: &ExecuteCommandCmd,
    ) -> ServiceResult<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        if cmd.is_pipe {
            let segments = pipe_parser::parse_pipe_expression(&cmd.command_id)?;
            for segment in &segments {
                if let Some(def) = self
                    .resolve_command_definition(cmd.actor, &segment.command_id)
                    .await?
                {
                    for cap in &def.required_capabilities {
                        if seen.insert(cap.clone()) {
                            out.push(cap.clone());
                        }
                    }
                }
            }
        } else if let Some(def) = self
            .resolve_command_definition(cmd.actor, &cmd.command_id)
            .await?
        {
            for cap in &def.required_capabilities {
                if seen.insert(cap.clone()) {
                    out.push(cap.clone());
                }
            }
        }
        Ok(out)
    }

    /// Returns `true` if dispatching `cmd` requires a `ConfirmationProof`.
    pub(super) async fn dispatch_requires_confirmation(
        &self,
        cmd: &ExecuteCommandCmd,
    ) -> ServiceResult<bool> {
        if cmd.is_pipe {
            let segments = pipe_parser::parse_pipe_expression(&cmd.command_id)?;
            for segment in &segments {
                let Some(def) = self
                    .resolve_command_definition(cmd.actor, &segment.command_id)
                    .await?
                else {
                    return Ok(false);
                };
                if def.requires_confirmation_for_initiator(cmd.initiator) {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        let Some(def) = self
            .resolve_command_definition(cmd.actor, &cmd.command_id)
            .await?
        else {
            return Ok(false);
        };
        Ok(def.requires_confirmation_for_initiator(cmd.initiator))
    }
}

impl DefaultCommandService {
    /// Track 1.2 timeout gate: `Err(TimedOut { until })` when `definition` is Mutating (or
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
