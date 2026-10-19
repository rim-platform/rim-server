use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
use super::pipe_parser::{self, PipeSegment};
use super::resolver::{CommandResolver, Resolved, expanded_steps_exceeded};
use super::user_commands;

/// The caller, context and user-command nesting depth of one dispatch, and the run it is part of.
#[derive(Clone)]
pub(super) struct ExecutionContext {
    pub actor: Actor,
    pub context: Option<CommandContextDto>,
    pub depth: usize,
    pub run: Arc<RunScope>,
}

/// One top-level run: the user commands its resolution read, which are the ones it runs, and
/// the count of builtin steps it has started, every pipe stage included.
pub(super) struct RunScope {
    user_commands: HashMap<String, UserCommandDto>,
    builtin_steps: AtomicUsize,
}

impl RunScope {
    pub(super) fn new(user_commands: HashMap<String, UserCommandDto>) -> Self {
        Self {
            user_commands,
            builtin_steps: AtomicUsize::new(0),
        }
    }

    /// Counts one more builtin step of the run.
    ///
    /// # Errors
    ///
    /// `Validation` once the run has started
    /// [`DefaultCommandService::MAX_EXPANDED_STEPS`] builtin steps.
    pub(super) fn start_builtin_step(&self) -> ServiceResult<()> {
        if self.builtin_steps.fetch_add(1, Ordering::Relaxed)
            >= DefaultCommandService::MAX_EXPANDED_STEPS
        {
            return Err(expanded_steps_exceeded("a command run"));
        }
        Ok(())
    }
}

/// What one `execute_command` call dispatches, resolved once for every gate and the run.
pub(super) enum Dispatch {
    /// One command; `None` when its id names no command the caller may run.
    Single(Option<Box<Resolved>>),
    /// The stages of a pipe, each with its command, `None` when the id names none.
    Pipe(Vec<(PipeSegment, Option<Resolved>)>),
}

impl Dispatch {
    /// Refuses a dispatch whose user commands nest deeper than the executor runs or that
    /// performs more than [`DefaultCommandService::MAX_EXPANDED_STEPS`] builtin steps, all
    /// pipe stages together.
    fn ensure_within_limits(&self) -> ServiceResult<()> {
        let (subject, resolved): (&str, Vec<&Resolved>) = match self {
            Self::Single(resolved) => ("a user command", resolved.as_deref().into_iter().collect()),
            Self::Pipe(stages) => (
                "a pipe",
                stages.iter().filter_map(|(_, r)| r.as_ref()).collect(),
            ),
        };
        if resolved
            .iter()
            .any(|r| r.depth > DefaultCommandService::MAX_RECURSION_DEPTH)
        {
            return Err(ServiceError::Validation(
                RECURSION_DEPTH_EXCEEDED.to_string(),
            ));
        }
        let expanded_steps = resolved
            .iter()
            .fold(0usize, |total, r| total.saturating_add(r.expanded_steps));
        if expanded_steps > DefaultCommandService::MAX_EXPANDED_STEPS {
            return Err(expanded_steps_exceeded(subject));
        }
        Ok(())
    }
}

const RECURSION_DEPTH_EXCEEDED: &str = "user command recursion depth exceeded";

impl DefaultCommandService {
    pub(super) fn builtin_definition(command_id: &str) -> Option<CommandDefinitionDto> {
        Self::builtin_commands()
            .into_iter()
            .find(|definition| definition.id == command_id)
    }

    /// The builtin `command_id`, or under test a definition injected in its place.
    pub(super) fn builtin_or_test_definition(
        &self,
        command_id: &str,
    ) -> Option<CommandDefinitionDto> {
        #[cfg(test)]
        if let Some(definition) = self
            .extra_builtins
            .iter()
            .find(|d| d.id == command_id)
            .cloned()
        {
            return Some(definition);
        }

        Self::builtin_definition(command_id)
    }

    /// Resolves `command_id` for `actor`: a builtin, or one of their enabled user commands.
    pub(super) async fn resolve_command_definition(
        &self,
        actor: Actor,
        command_id: &str,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        Ok(CommandResolver::new(self, actor)
            .resolve(command_id)
            .await?
            .map(|resolved| resolved.definition))
    }

    /// Refuses `steps` for the actor's user command `command_key` unless, with the actor's
    /// stored user commands, no chain of user commands through it is deeper than the executor
    /// runs, neither it nor a stored command calling it expands to more than
    /// [`Self::MAX_EXPANDED_STEPS`] builtin steps, and each step resolves to a backend command
    /// without a cycle. A new command (`creating`) is also refused once the actor has
    /// [`Self::MAX_USER_COMMANDS_PER_USER`] user commands.
    ///
    /// # Errors
    ///
    /// `Validation` naming the limit or the step refused; a repository error.
    pub(super) async fn validate_user_command_for_actor(
        &self,
        actor: Actor,
        command_key: &str,
        steps: &[UserCommandStepDto],
        creating: bool,
    ) -> ServiceResult<()> {
        let stored = self
            .repos
            .user_command_repo
            .list_for_user(actor.user_id)
            .await?
            .into_iter()
            .map(Self::parse_user_command)
            .collect::<ServiceResult<Vec<_>>>()?;
        if creating && stored.len() >= Self::MAX_USER_COMMANDS_PER_USER {
            return Err(ServiceError::Validation(format!(
                "a user may have at most {} user commands",
                Self::MAX_USER_COMMANDS_PER_USER
            )));
        }
        if user_commands::longest_chain_through(&stored, command_key, steps)
            > Self::MAX_RECURSION_DEPTH
        {
            return Err(ServiceError::Validation(
                RECURSION_DEPTH_EXCEEDED.to_string(),
            ));
        }
        if user_commands::most_expanded_steps_through(&stored, command_key, steps)
            > Self::MAX_EXPANDED_STEPS
        {
            return Err(expanded_steps_exceeded("a user command"));
        }
        CommandResolver::with_user_commands(self, actor, &stored)
            .check_steps(command_key, steps)
            .await
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
        stages: Vec<(PipeSegment, Option<Resolved>)>,
        context: Option<CommandContextDto>,
        run: Arc<RunScope>,
    ) -> ServiceResult<CommandResultDto> {
        use rim_services::command::dto::PipeStageResultDto;

        for (i, (segment, resolved)) in stages.iter().enumerate() {
            let definition = &resolved
                .as_ref()
                .ok_or_else(|| {
                    ServiceError::Validation(format!(
                        "pipe stage {} ('{}') refers to an unknown command",
                        i + 1,
                        segment.command_id,
                    ))
                })?
                .definition;

            if definition.execution_target == CommandExecutionTargetDto::DesktopLocal {
                return Err(ServiceError::Validation(format!(
                    "piped commands must all target the same execution runtime; \
                     stage {} ('{}') is desktop-local",
                    i + 1,
                    segment.command_id,
                )));
            }
        }
        let segments: Vec<PipeSegment> = stages.into_iter().map(|(segment, _)| segment).collect();

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
                        run: Arc::clone(&run),
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
            ctx.run.start_builtin_step()?;
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
            ctx.run.start_builtin_step()?;
            // Role and timeout gates for nested dispatch: user-command steps and pipe stages
            // reach builtins here without passing the top-level gates in `execute_command_inner`.
            self.ensure_role_admits(&ctx.actor, command_id, args.as_ref(), ctx.context.as_ref())
                .await?;
            self.ensure_command_not_timed_out(&definition, &ctx.actor, ctx.context.as_ref())
                .await?;
            return self
                .execute_builtin(ctx.actor, command_id, args, ctx.context)
                .await;
        }

        let user_command = ctx
            .run
            .user_commands
            .get(command_id)
            .cloned()
            .ok_or(ServiceError::NotFound)?;

        self.execute_user_command_recursive(user_command, ctx).await
    }

    /// Inner dispatch body for `execute_command`, given `resolver` and, for a single command,
    /// its resolution `top_level`. The public trait method wraps this with the audit emit so
    /// every branch gets exactly one audit line.
    pub(super) async fn execute_command_inner(
        &self,
        cmd: ExecuteCommandCmd,
        top_level: ServiceResult<Option<Resolved>>,
        mut resolver: CommandResolver<'_>,
    ) -> ServiceResult<CommandResultDto> {
        if !cmd.is_pipe && cmd.command_id.contains('|') {
            return Err(ServiceError::Validation(
                "command_id contains pipe character '|' but is_pipe was not set to true".into(),
            ));
        }

        let dispatch = if cmd.is_pipe {
            let segments = pipe_parser::parse_pipe_expression(&cmd.command_id)?;
            let mut stages = Vec::with_capacity(segments.len());
            for segment in segments {
                let resolved = resolver.resolve(&segment.command_id).await?;
                stages.push((segment, resolved));
            }
            Dispatch::Pipe(stages)
        } else {
            Dispatch::Single(top_level?.map(Box::new))
        };
        dispatch.ensure_within_limits()?;
        let run = Arc::new(RunScope::new(resolver.into_user_commands()));

        // ── Capability gate ─────────────────────────────────────────────
        let required_caps = dispatch.required_capabilities();
        if !required_caps.is_empty() {
            let workspace_id = cmd.context.as_ref().and_then(|c| c.workspace_id);
            for cap in &required_caps {
                self.services
                    .capability_service
                    .check(&cmd.actor, workspace_id, cap)
                    .await?;
            }
        }

        // ── Role gate ───────────────────────────────────────────────────
        // Before the confirmation gate, so a caller whose role cannot run the command is
        // refused rather than asked to confirm it.
        if !cmd.is_pipe {
            self.ensure_role_admits(
                &cmd.actor,
                &cmd.command_id,
                cmd.args.as_ref(),
                cmd.context.as_ref(),
            )
            .await?;
        }

        // ── Confirmation gate ────────────────────────────────────────────
        if dispatch.requires_confirmation(cmd.initiator) {
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

        let top_level_def = match &dispatch {
            Dispatch::Single(resolved) => resolved.as_ref().map(|r| &r.definition),
            Dispatch::Pipe(_) => None,
        };
        // ── Timeout gate ────────────────────────────────────────────────
        // A timed-out member cannot run Mutating commands in that workspace; reads still work.
        // Also before the rate limiter, so a refused command consumes no slot.
        if let Some(def) = top_level_def {
            self.ensure_command_not_timed_out(def, &cmd.actor, cmd.context.as_ref())
                .await?;
        }

        // ── Rate-limit gate (runs after capability + confirmation) ──────
        if let Some(def) = top_level_def
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

        match dispatch {
            Dispatch::Pipe(stages) => {
                self.execute_piped(cmd.actor, stages, cmd.context, run)
                    .await
            }
            Dispatch::Single(None) => Err(ServiceError::NotFound),
            Dispatch::Single(Some(_)) => {
                self.execute_resolved_command_recursive(
                    &cmd.command_id,
                    cmd.args,
                    ExecutionContext {
                        actor: cmd.actor,
                        context: cmd.context,
                        depth: 0,
                        run,
                    },
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_refuses_the_builtin_step_past_the_limit() {
        let run = RunScope::new(HashMap::new());
        for _ in 0..DefaultCommandService::MAX_EXPANDED_STEPS {
            run.start_builtin_step()
                .expect("a step within the limit starts");
        }

        let err = run.start_builtin_step().unwrap_err();

        assert!(
            matches!(&err, ServiceError::Validation(msg) if msg == "a command run expands to more than 100 steps"),
            "{err:?}"
        );
    }
}
