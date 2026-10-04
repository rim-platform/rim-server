use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto};
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::{ServiceError, ServiceResult};

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
    /// Track 0.4 list filter: drops every command whose `feature` is not enabled for the
    /// context's workspace, with one feature lookup for the whole list. Without a workspace
    /// context every feature-gated command is dropped (it could not run anyway).
    pub(super) async fn retain_enabled_features(
        &self,
        commands: &mut Vec<CommandDefinitionDto>,
        context: Option<&CommandContextDto>,
    ) -> ServiceResult<()> {
        if commands.iter().all(|c| c.feature.is_none()) {
            return Ok(());
        }
        let enabled = match context.and_then(|c| c.workspace_id) {
            Some(server_id) => {
                self.services
                    .workspace_feature_service
                    .enabled_features(server_id)
                    .await?
            }
            None => Vec::new(),
        };
        commands.retain(|c| c.feature.is_none_or(|f| enabled.contains(&f)));
        Ok(())
    }

    /// Track 0.4 execution gate: `Err(FeatureDisabled)` when `definition` belongs to a feature
    /// that is off for the context's workspace, or when there is no workspace context at all.
    pub(super) async fn ensure_command_feature_enabled(
        &self,
        definition: &CommandDefinitionDto,
        context: Option<&CommandContextDto>,
    ) -> ServiceResult<()> {
        let Some(feature) = definition.feature else {
            return Ok(());
        };
        let enabled = match context.and_then(|c| c.workspace_id) {
            Some(server_id) => {
                self.services
                    .workspace_feature_service
                    .is_enabled(server_id, feature)
                    .await?
            }
            None => false,
        };
        if enabled {
            Ok(())
        } else {
            Err(ServiceError::feature_disabled(feature))
        }
    }
}
