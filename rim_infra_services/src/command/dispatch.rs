use async_recursion::async_recursion;
use serde_json::Value;

use rim_services::command::confirmation::{issue_in_band, verify_in_band};
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto, CommandResultDto,
    CommandResultStatusDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::executor;
use super::pipe_parser;
use super::user_commands;

/// The caller, context and user-command nesting depth of one dispatch.
#[derive(Clone)]
pub(super) struct ExecutionContext {
    pub actor: Actor,
    pub context: Option<CommandContextDto>,
    pub depth: usize,
}

const RECURSION_DEPTH_EXCEEDED: &str = "user command recursion depth exceeded";

impl DefaultCommandService {
    pub(super) fn builtin_definition(command_id: &str) -> Option<CommandDefinitionDto> {
        Self::builtin_commands()
            .into_iter()
            .find(|definition| definition.id == command_id)
    }

    #[async_recursion]
    pub(super) async fn derive_user_command_definition(
        &self,
        actor: Actor,
        user_command: UserCommandDto,
        seen: &mut std::collections::BTreeSet<String>,
    ) -> ServiceResult<CommandDefinitionDto> {
        if !seen.insert(user_command.command_key.clone()) {
            return Err(ServiceError::Validation(
                "user command recursion cycle detected".to_string(),
            ));
        }

        let mut required_context = std::collections::BTreeSet::new();
        let mut required_capabilities = std::collections::BTreeSet::new();
        let mut keybind_capable = true;
        let mut dangerous = false;
        let mut step_sensitivities = Vec::with_capacity(user_command.steps.len());

        for step in &user_command.steps {
            let definition = self
                .resolve_command_definition_within(actor, &step.command_id, seen)
                .await?
                .ok_or_else(|| {
                    ServiceError::Validation(format!("unknown command step `{}`", step.command_id))
                })?;

            if definition.execution_target != CommandExecutionTargetDto::Backend {
                return Err(ServiceError::Validation(
                    "user commands may only compose backend commands in v1".to_string(),
                ));
            }

            required_context.extend(definition.required_context);
            required_capabilities.extend(definition.required_capabilities);
            keybind_capable &= definition.keybind_capable;
            dangerous |= definition.dangerous;
            step_sensitivities.push(definition.sensitivity);
        }

        seen.remove(&user_command.command_key);

        Ok(user_command.as_command_definition_with_metadata(
            CommandExecutionTargetDto::Backend,
            required_context.into_iter().collect(),
            required_capabilities.into_iter().collect(),
            keybind_capable,
            dangerous,
            &step_sensitivities,
        ))
    }

    #[async_recursion]
    async fn get_user_command_definition(
        &self,
        actor: Actor,
        command_id: &str,
        seen: &mut std::collections::BTreeSet<String>,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        let user_command = self
            .repos
            .user_command_repo
            .get_by_key_for_user(actor.user_id, command_id)
            .await?;

        let Some(user_command) = user_command else {
            return Ok(None);
        };
        let dto = Self::parse_user_command(user_command)?;
        if !dto.is_enabled {
            return Ok(None);
        }

        Ok(Some(
            self.derive_user_command_definition(actor, dto, seen)
                .await?,
        ))
    }

    pub(super) async fn resolve_command_definition(
        &self,
        actor: Actor,
        command_id: &str,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        self.resolve_command_definition_within(
            actor,
            command_id,
            &mut std::collections::BTreeSet::new(),
        )
        .await
    }

    /// Resolves `command_id` while the user commands in `seen` are being resolved above it;
    /// reaching one of them again is a cycle.
    #[async_recursion]
    async fn resolve_command_definition_within(
        &self,
        actor: Actor,
        command_id: &str,
        seen: &mut std::collections::BTreeSet<String>,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        #[cfg(test)]
        if let Some(definition) = self
            .extra_builtins
            .iter()
            .find(|d| d.id == command_id)
            .cloned()
        {
            return Ok(Some(definition));
        }

        if let Some(definition) = Self::builtin_definition(command_id) {
            return Ok(Some(definition));
        }

        self.get_user_command_definition(actor, command_id, seen)
            .await
    }

    #[async_recursion]
    pub(super) async fn validate_user_command_steps_for_actor(
        &self,
        actor: Actor,
        command_key: &str,
        steps: &[UserCommandStepDto],
        seen: &mut std::collections::BTreeSet<String>,
    ) -> ServiceResult<()> {
        user_commands::validate_user_command_step_shape(command_key, steps)?;

        if !seen.insert(command_key.to_string()) {
            return Err(ServiceError::Validation(
                "user command recursion cycle detected".to_string(),
            ));
        }

        for step in steps {
            let definition = self
                .resolve_command_definition(actor, &step.command_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::Validation(format!("unknown command step `{}`", step.command_id))
                })?;

            if definition.execution_target != CommandExecutionTargetDto::Backend {
                return Err(ServiceError::Validation(
                    "user commands may only compose backend commands in v1".to_string(),
                ));
            }

            if step.command_id.starts_with("user.") {
                let referenced = self
                    .repos
                    .user_command_repo
                    .get_by_key_for_user(actor.user_id, &step.command_id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::Validation(format!(
                            "unknown command step `{}`",
                            step.command_id
                        ))
                    })?;
                let referenced = Self::parse_user_command(referenced)?;

                self.validate_user_command_steps_for_actor(
                    actor,
                    &referenced.command_key,
                    &referenced.steps,
                    seen,
                )
                .await?;
            }
        }

        seen.remove(command_key);
        Ok(())
    }

    /// Refuses `steps` for `command_key` when, with the actor's stored user commands, they
    /// would form a chain of user commands deeper than the executor runs.
    pub(super) async fn ensure_user_command_chain_within_depth(
        &self,
        actor: Actor,
        command_key: &str,
        steps: &[UserCommandStepDto],
    ) -> ServiceResult<()> {
        let stored = self
            .repos
            .user_command_repo
            .list_for_user(actor.user_id)
            .await?
            .into_iter()
            .map(Self::parse_user_command)
            .collect::<ServiceResult<Vec<_>>>()?;
        if user_commands::longest_chain_through(&stored, command_key, steps)
            > Self::MAX_RECURSION_DEPTH
        {
            return Err(ServiceError::Validation(
                RECURSION_DEPTH_EXCEEDED.to_string(),
            ));
        }
        Ok(())
    }

    pub(super) async fn execute_builtin(
        &self,
        actor: Actor,
        command_id: &str,
        args: Option<Value>,
        context: Option<CommandContextDto>,
    ) -> ServiceResult<CommandResultDto> {
        executor::execute_builtin_command(&self.services, actor, command_id, args, context).await
    }

    #[async_recursion]
    pub(super) async fn execute_user_command_recursive(
        &self,
        user_command: UserCommandDto,
        ctx: ExecutionContext,
    ) -> ServiceResult<CommandResultDto> {
        if ctx.depth >= Self::MAX_RECURSION_DEPTH {
            return Err(ServiceError::Validation(
                RECURSION_DEPTH_EXCEEDED.to_string(),
            ));
        }

        let mut payload = serde_json::Map::new();
        for step in user_command.steps {
            let result = self
                .execute_resolved_command_recursive(
                    &step.command_id,
                    step.args,
                    ExecutionContext {
                        depth: ctx.depth + 1,
                        ..ctx.clone()
                    },
                )
                .await?;
            if let Some(step_payload) = result.payload {
                payload.insert(step.command_id, step_payload);
            }
        }

        Ok(CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: format!("user command `{}` executed", user_command.command_key),
            payload: Some(Value::Object(payload)),
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        })
    }

    pub(super) async fn execute_piped(
        &self,
        actor: Actor,
        pipe_expression: &str,
        context: Option<CommandContextDto>,
    ) -> ServiceResult<CommandResultDto> {
        use rim_services::command::dto::PipeStageResultDto;

        let segments = pipe_parser::parse_pipe_expression(pipe_expression)?;

        for (i, segment) in segments.iter().enumerate() {
            let definition = self
                .resolve_command_definition(actor, &segment.command_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::Validation(format!(
                        "pipe stage {} ('{}') refers to an unknown command",
                        i + 1,
                        segment.command_id,
                    ))
                })?;

            if definition.execution_target == CommandExecutionTargetDto::DesktopLocal {
                return Err(ServiceError::Validation(format!(
                    "piped commands must all target the same execution runtime; \
                     stage {} ('{}') is desktop-local",
                    i + 1,
                    segment.command_id,
                )));
            }
        }

        let mut previous_payload: Option<serde_json::Value> = None;
        let mut all_results = serde_json::Map::new();
        let mut pipeline_stages: Vec<PipeStageResultDto> = Vec::with_capacity(segments.len());

        for (i, segment) in segments.iter().enumerate() {
            let merged_args =
                pipe_parser::merge_pipe_args(previous_payload.as_ref(), segment.args.as_ref());

            let prev_command_id = if i > 0 {
                Some(segments[i - 1].command_id.as_str())
            } else {
                None
            };

            let result = self
                .execute_resolved_command_recursive(
                    &segment.command_id,
                    merged_args,
                    ExecutionContext {
                        actor,
                        context: context.clone(),
                        depth: 0,
                    },
                )
                .await;

            match result {
                Ok(result) => {
                    pipeline_stages.push(PipeStageResultDto {
                        stage_index: i as u32,
                        command_id: segment.command_id.clone(),
                        status: CommandResultStatusDto::Success,
                        message: result.message.clone(),
                        payload: result.payload.clone(),
                    });

                    previous_payload = result.payload.clone();
                    all_results.insert(
                        format!("step_{}", i + 1),
                        serde_json::json!({
                            "command_id": segment.command_id,
                            "status": "Success",
                            "payload": result.payload,
                        }),
                    );
                }
                Err(e) => {
                    let error_message = match &e {
                        ServiceError::Validation(msg) if msg.contains("missing string arg") => {
                            let prev_keys = previous_payload
                                .as_ref()
                                .and_then(|p| p.as_object())
                                .map(|obj| obj.keys().cloned().collect::<Vec<_>>().join(", "))
                                .unwrap_or_else(|| "(none)".to_string());
                            format!(
                                "pipe stage {} ('{}') {} — the previous stage{} produced: {{{}}}",
                                i + 1,
                                segment.command_id,
                                msg,
                                prev_command_id
                                    .map(|p| format!(" ('{}')", p))
                                    .unwrap_or_default(),
                                prev_keys,
                            )
                        }
                        _ => {
                            let context_prefix = format!(
                                "pipe stage {} ('{}') failed: ",
                                i + 1,
                                segment.command_id,
                            );
                            match &e {
                                ServiceError::Validation(msg) => format!("{context_prefix}{msg}"),
                                ServiceError::Conflict(msg) => format!("{context_prefix}{msg}"),
                                other => format!("{context_prefix}{other}"),
                            }
                        }
                    };

                    pipeline_stages.push(PipeStageResultDto {
                        stage_index: i as u32,
                        command_id: segment.command_id.clone(),
                        status: CommandResultStatusDto::Error,
                        message: error_message.clone(),
                        payload: None,
                    });

                    return Ok(CommandResultDto {
                        status: CommandResultStatusDto::Error,
                        message: error_message,
                        payload: Some(Value::Object(all_results)),
                        follow_up_commands: vec![],
                        pipeline_stages: Some(pipeline_stages),
                        confirmation_required: None,
                    });
                }
            }
        }

        Ok(CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: format!("piped {} commands successfully", segments.len()),
            payload: Some(Value::Object(all_results)),
            follow_up_commands: vec![],
            pipeline_stages: Some(pipeline_stages),
            confirmation_required: None,
        })
    }

    #[async_recursion]
    pub(super) async fn execute_resolved_command_recursive(
        &self,
        command_id: &str,
        args: Option<Value>,
        ctx: ExecutionContext,
    ) -> ServiceResult<CommandResultDto> {
        #[cfg(test)]
        if self.extra_builtins.iter().any(|d| d.id == command_id) {
            return Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: format!("stub: {command_id}"),
                payload: None,
                follow_up_commands: vec![],
                pipeline_stages: None,
                confirmation_required: None,
            });
        }

        if let Some(definition) = Self::builtin_definition(command_id) {
            // Timeout gate for nested dispatch: user-command steps and pipe stages reach
            // builtins here without passing the top-level gate in `execute_command_inner`.
            self.ensure_command_not_timed_out(&definition, &ctx.actor, ctx.context.as_ref())
                .await?;
            return self
                .execute_builtin(ctx.actor, command_id, args, ctx.context)
                .await;
        }

        let user_command = self
            .repos
            .user_command_repo
            .get_by_key_for_user(ctx.actor.user_id, command_id)
            .await?
            .map(Self::parse_user_command)
            .transpose()?
            .ok_or(ServiceError::NotFound)?;

        self.execute_user_command_recursive(user_command, ctx).await
    }

    /// Inner dispatch body for `execute_command`. The public trait method wraps
    /// this with the audit emit so every branch gets exactly one audit line.
    pub(super) async fn execute_command_inner(
        &self,
        cmd: ExecuteCommandCmd,
    ) -> ServiceResult<CommandResultDto> {
        if !cmd.is_pipe && cmd.command_id.contains('|') {
            return Err(ServiceError::Validation(
                "command_id contains pipe character '|' but is_pipe was not set to true".into(),
            ));
        }

        // ── Capability gate ─────────────────────────────────────────────
        let required_caps = self.dispatch_required_capabilities(&cmd).await?;
        if !required_caps.is_empty() {
            let workspace_id = cmd.context.as_ref().and_then(|c| c.workspace_id);
            for cap in &required_caps {
                self.services
                    .capability_service
                    .check(&cmd.actor, workspace_id, cap)
                    .await?;
            }
        }

        // ── Confirmation gate ────────────────────────────────────────────
        let needs_confirmation = self.dispatch_requires_confirmation(&cmd).await?;
        if needs_confirmation {
            match &cmd.confirmation {
                None => {
                    let challenge = issue_in_band(
                        self.confirmation_store.as_ref(),
                        &cmd.actor,
                        &cmd.command_id,
                        &cmd.args,
                        &cmd.context,
                    );
                    return Ok(CommandResultDto {
                        status: CommandResultStatusDto::ConfirmationRequired,
                        message: "confirmation required for dangerous command".to_string(),
                        payload: None,
                        follow_up_commands: vec![],
                        pipeline_stages: None,
                        confirmation_required: Some(challenge),
                    });
                }
                Some(proof) => {
                    if let Err(e) = verify_in_band(
                        self.confirmation_store.as_ref(),
                        proof,
                        &cmd.actor,
                        &cmd.command_id,
                        &cmd.args,
                        &cmd.context,
                    ) {
                        if e.is_stale() {
                            return Err(ServiceError::ConfirmationStale);
                        }
                        return Err(ServiceError::ConfirmationFailed(e.to_string()));
                    }
                }
            }
        }

        let top_level_def = if cmd.is_pipe {
            None
        } else {
            self.resolve_command_definition(cmd.actor, &cmd.command_id)
                .await
                .ok()
                .flatten()
        };
        // ── Timeout gate ────────────────────────────────────────────────
        // A timed-out member cannot run Mutating commands in that workspace; reads still work.
        // Also before the rate limiter, so a refused command consumes no slot.
        if let Some(def) = &top_level_def {
            self.ensure_command_not_timed_out(def, &cmd.actor, cmd.context.as_ref())
                .await?;
        }

        // ── Rate-limit gate (runs after capability + confirmation) ──────
        if let Some(def) = &top_level_def
            && let Some(limit) = def.rate_limit_per_minute
        {
            let decision = self.rate_limiter.check_and_record(
                cmd.actor.user_id,
                &cmd.command_id,
                limit,
                chrono::Utc::now(),
            );
            if let rim_services::command::rate_limit::RateLimitDecision::Limited {
                retry_after_seconds,
            } = decision
            {
                tracing::info!(
                    target: "audit",
                    event = "rate_limited",
                    actor_id = %cmd.actor.user_id,
                    command_id = %cmd.command_id,
                    limit = limit,
                    retry_after = retry_after_seconds,
                );
                crate::metrics::rate_limited("command");
                return Err(ServiceError::RateLimited {
                    retry_after_seconds,
                });
            }
        }

        if cmd.is_pipe {
            return self
                .execute_piped(cmd.actor, &cmd.command_id, cmd.context)
                .await;
        }

        self.execute_resolved_command_recursive(
            &cmd.command_id,
            cmd.args,
            ExecutionContext {
                actor: cmd.actor,
                context: cmd.context,
                depth: 0,
            },
        )
        .await
    }
}
