mod discover;
mod dispatch;
mod executor;
mod gates;
mod pipe_parser;
mod rate_limiter;
pub(crate) mod registry;
mod resolver;
mod surface;
mod user_commands;
mod visibility;

// Re-export the startup AI-callable inventory audit so `rim_server::app` can wire
// it without exposing the rest of the registry internals.
pub use registry::emit_ai_callable_inventory_audit;

use async_trait::async_trait;

use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand};
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::bounds::{
    MAX_DESCRIPTION_CHARS, MAX_EXECUTE_ARGS_BYTES, MAX_PIPE_EXPRESSION_BYTES, bound_audit_args,
    ensure_args_size, ensure_chars, ensure_command_id_size,
};
use rim_services::command::confirmation::{
    ConfirmationProof, ConfirmationStore, shared_in_memory_store,
};
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandResultDto, CommandResultStatusDto,
    CommandSurfaceDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    GetUserCommandQuery, ListCommandsQuery, ListMacroStepCommandsQuery, ListUserCommandsQuery,
    UpdateUserCommandCmd,
};
use rim_services::command::sanitize::{
    MAX_INTENT_SAMPLE_LEN, MAX_NL_DESCRIPTION_LEN, sanitize_nl_text,
};
use rim_services::command::sensitivity::{CommandSensitivityDto, sanitize_args_for_audit};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::message::service::MessageService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::CommandRepos;
use crate::command::dispatch::Dispatch;
use crate::command::resolver::CommandResolver;
use crate::command::user_commands::{persisted_steps, steps_with_stored_asks};

#[derive(Clone)]
pub struct CommandServices {
    pub auth_service: std::sync::Arc<dyn rim_services::auth::service::AuthService>,
    pub capability_service:
        std::sync::Arc<dyn rim_services::capability::service::CapabilityService>,
    pub conversation_service: std::sync::Arc<dyn ConversationService>,
    pub message_service: std::sync::Arc<dyn MessageService>,
    pub message_reaction_service:
        std::sync::Arc<dyn rim_services::message_reaction::service::MessageReactionService>,
    pub pin_service: std::sync::Arc<dyn rim_services::pin::service::PinService>,
    pub read_state_service: std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>,
    pub server_member_service:
        std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>,
    pub user_service: std::sync::Arc<dyn rim_services::user::service::UserService>,
    pub workspace_state_service:
        std::sync::Arc<dyn rim_services::workspace_state::service::WorkspaceStateService>,
    /// Moderation builtins (`admin.kick`, `chat.set_slow_mode`, ...) dispatch to it,
    /// and the timeout gate asks it whether the actor is timed out before any Mutating command.
    pub moderation_service:
        std::sync::Arc<dyn rim_services::moderation::service::ModerationService>,
    /// `notifications.set_level`, `notifications.dnd` and
    /// `notifications.mark_all_read` dispatch to it.
    pub notification_service:
        std::sync::Arc<dyn rim_services::notification::service::NotificationService>,
    /// `notifications.mute_conversation` and `notifications.unmute_conversation` dispatch to it.
    pub notification_preference_service: std::sync::Arc<
        dyn rim_services::notification_preference::service::NotificationPreferenceService,
    >,
    /// Database audit sink. `None` in test/unit contexts
    /// that don't wire up a DB. When `Some`, every `execute_command` result
    /// writes a structured row alongside the `tracing::info!(target: "audit")`
    /// line. Both sinks are kept — the tracing line is for low-cardinality
    /// monitoring (grep/tail); the DB row is for queryable forensics.
    pub audit_log_service: Option<std::sync::Arc<dyn AuditLogService>>,
}

#[derive(Clone)]
pub struct DefaultCommandService {
    pub(super) repos: CommandRepos,
    pub(super) services: CommandServices,
    /// Holds the issued confirmation tokens for dangerous commands. The
    /// store in use is in-memory, so tokens are valid only on the node
    /// that issued them; another store can replace it behind the same trait.
    pub(super) confirmation_store: std::sync::Arc<dyn ConfirmationStore>,
    /// In-memory per-(actor, command_id) sliding-window rate limiter.
    /// Single-node only: each process keeps its own windows.
    pub(super) rate_limiter: rate_limiter::RateLimiter,
    /// This server's direct space, where a guest may run some commands that elsewhere need a
    /// member.
    pub(super) direct_space_id: Option<rim_core::server::model::ServerId>,
    /// Extra command definitions injected by tests. Empty in production.
    #[cfg(test)]
    pub(super) extra_builtins: Vec<CommandDefinitionDto>,
}

impl DefaultCommandService {
    /// User commands one chain of nested user commands may hold.
    pub(super) const MAX_RECURSION_DEPTH: usize = 8;
    /// Steps one user command may list.
    pub(super) const MAX_USER_COMMAND_STEPS: usize = 20;
    /// User commands one user may keep.
    pub(super) const MAX_USER_COMMANDS_PER_USER: usize = 100;
    /// Builtin steps one run may perform, through every nested user command and pipe stage.
    pub(super) const MAX_EXPANDED_STEPS: usize = 100;
    /// Characters of a user command's title.
    pub(super) const MAX_USER_COMMAND_TITLE_CHARS: usize = 128;
    /// Intent samples one macro may keep.
    pub(super) const MAX_USER_COMMAND_INTENT_SAMPLES: usize = 10;
    /// The refusal of `aiCallable` on a macro the AI agent may not run.
    pub(super) const AGENT_RUNNABLE_RULE: &'static str = "only a macro whose steps are all commands the AI agent may run, run on the server with every argument fixed, can be run by the AI agent";

    pub fn new(repos: CommandRepos, services: CommandServices) -> Self {
        Self::with_confirmation_store(repos, services, shared_in_memory_store())
    }

    /// Constructor with explicit confirmation store. Tests can inject a
    /// shared store to assert issue/verify behavior; production wiring
    /// uses the default in-memory store via [`Self::new`].
    pub fn with_confirmation_store(
        repos: CommandRepos,
        services: CommandServices,
        confirmation_store: std::sync::Arc<dyn ConfirmationStore>,
    ) -> Self {
        Self {
            repos,
            services,
            confirmation_store,
            rate_limiter: rate_limiter::RateLimiter::new(),
            direct_space_id: None,
            #[cfg(test)]
            extra_builtins: vec![],
        }
    }

    /// Names this server's direct space.
    pub fn with_direct_space(mut self, direct_space_id: rim_core::server::model::ServerId) -> Self {
        self.direct_space_id = Some(direct_space_id);
        self
    }

    /// Test-only constructor that injects synthetic command definitions into
    /// the builtin registry.
    #[cfg(test)]
    pub fn with_extra_builtins(
        repos: CommandRepos,
        services: CommandServices,
        extra_builtins: Vec<CommandDefinitionDto>,
    ) -> Self {
        Self {
            repos,
            services,
            confirmation_store: shared_in_memory_store(),
            rate_limiter: rate_limiter::RateLimiter::new(),
            direct_space_id: None,
            extra_builtins,
        }
    }

    pub(super) fn builtin_commands() -> Vec<CommandDefinitionDto> {
        registry::builtin_commands()
    }

    /// Refuses an `execute_command` call whose command id is longer than 256 bytes, whose pipe
    /// expression is longer than [`MAX_PIPE_EXPRESSION_BYTES`], or whose arguments serialize to
    /// more than [`MAX_EXECUTE_ARGS_BYTES`].
    ///
    /// # Errors
    ///
    /// `Validation` naming the bound.
    fn ensure_call_within_bounds(cmd: &ExecuteCommandCmd) -> ServiceResult<()> {
        if cmd.is_pipe {
            if cmd.command_id.len() > MAX_PIPE_EXPRESSION_BYTES {
                return Err(ServiceError::Validation(format!(
                    "a pipe expression may be at most {MAX_PIPE_EXPRESSION_BYTES} bytes"
                )));
            }
        } else {
            ensure_command_id_size(&cmd.command_id, "a command id")?;
        }
        ensure_args_size(
            cmd.args.as_ref(),
            MAX_EXECUTE_ARGS_BYTES,
            "command arguments",
        )
    }

    /// Refuses a user command title that is empty or longer than
    /// [`Self::MAX_USER_COMMAND_TITLE_CHARS`], or a description longer than
    /// [`MAX_DESCRIPTION_CHARS`].
    ///
    /// # Errors
    ///
    /// `Validation` naming the field.
    fn validate_user_command_text(title: &str, description: &str) -> ServiceResult<()> {
        if title.trim().is_empty() {
            return Err(ServiceError::Validation(
                "user command title must be non-empty".to_string(),
            ));
        }
        ensure_chars(
            title.trim(),
            Self::MAX_USER_COMMAND_TITLE_CHARS,
            "a user command title",
        )?;
        ensure_chars(
            description.trim(),
            MAX_DESCRIPTION_CHARS,
            "a user command description",
        )
    }

    /// Refuses a macro's natural-language description longer than [`MAX_NL_DESCRIPTION_LEN`]
    /// characters, more than [`Self::MAX_USER_COMMAND_INTENT_SAMPLES`] intent samples or one
    /// longer than [`MAX_INTENT_SAMPLE_LEN`] characters, and any of them that looks like an
    /// instruction to a model. Returns them cleaned of control characters and trimmed.
    ///
    /// # Errors
    ///
    /// `Validation` naming the field.
    fn sanitized_agent_text(
        nl_description: &str,
        intent_samples: &[String],
    ) -> ServiceResult<(String, Vec<String>)> {
        ensure_chars(
            nl_description,
            MAX_NL_DESCRIPTION_LEN,
            "a macro's natural-language description",
        )?;
        let nl_description =
            sanitize_nl_text(nl_description, MAX_NL_DESCRIPTION_LEN).map_err(|err| {
                ServiceError::Validation(format!("a macro's natural-language description: {err}"))
            })?;
        if intent_samples.len() > Self::MAX_USER_COMMAND_INTENT_SAMPLES {
            return Err(ServiceError::Validation(format!(
                "a macro may have at most {} intent samples",
                Self::MAX_USER_COMMAND_INTENT_SAMPLES
            )));
        }
        let intent_samples = intent_samples
            .iter()
            .map(|sample| {
                ensure_chars(sample, MAX_INTENT_SAMPLE_LEN, "a macro intent sample")?;
                sanitize_nl_text(sample, MAX_INTENT_SAMPLE_LEN).map_err(|err| {
                    ServiceError::Validation(format!("a macro intent sample: {err}"))
                })
            })
            .collect::<ServiceResult<Vec<_>>>()?;
        Ok((nl_description, intent_samples))
    }

    pub(super) fn validate_user_command_steps(
        command_key: &str,
        steps: &[UserCommandStepDto],
        stored: Option<&UserCommandDto>,
    ) -> ServiceResult<()> {
        user_commands::validate_user_command_steps(
            command_key,
            steps,
            stored,
            Self::builtin_definition,
        )
    }

    pub(super) fn command_matches_surface(
        definition: &CommandDefinitionDto,
        surface: CommandSurfaceDto,
        context: Option<&CommandContextDto>,
    ) -> bool {
        surface::command_matches_surface(definition, surface, context)
    }

    pub(super) fn parse_user_command(user_command: UserCommand) -> ServiceResult<UserCommandDto> {
        user_commands::parse_user_command(user_command)
    }
}

/// What the audit keeps of a call's error: its message, or a fixed kind for a pipe expression that
/// did not parse, since that message can quote a word of the expression.
fn audited_error(err: &ServiceError, unparsed_pipe: bool) -> String {
    if unparsed_pipe && matches!(err, ServiceError::Validation(_)) {
        pipe_parser::UNPARSED_PIPE_ERROR_KIND.to_string()
    } else {
        err.to_string()
    }
}

#[async_trait]
impl CommandService for DefaultCommandService {
    async fn list_commands(
        &self,
        query: ListCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        let context = query.context.as_ref();
        let mut commands = Self::builtin_commands();
        let user_commands = self
            .repos
            .user_command_repo
            .list_for_user(query.actor.user_id)
            .await?
            .into_iter()
            .map(Self::parse_user_command)
            .collect::<ServiceResult<Vec<_>>>()?;
        let user_steps: visibility::UserCommandSteps = user_commands
            .iter()
            .map(|dto| {
                let steps = dto.steps.iter().map(|s| s.command_id.clone()).collect();
                (dto.command_key.clone(), steps)
            })
            .collect();

        let mut resolver = CommandResolver::with_user_commands(self, query.actor, &user_commands);
        for dto in user_commands.iter().filter(|dto| dto.is_enabled) {
            match resolver.resolve_user(dto).await {
                Ok(resolved) => commands.push(resolved.definition),
                Err(err) => tracing::warn!(
                    event = "user_command_unlisted",
                    command_id = %dto.command_key,
                    error = %err,
                    "user command left out of the listing"
                ),
            }
        }

        if query.surface == CommandSurfaceDto::Ai {
            commands = self
                .filter_ai_surface(&query.actor, context, commands, &user_steps)
                .await?;
        } else {
            commands.retain(|definition| {
                Self::command_matches_surface(definition, query.surface, context)
            });
            commands = self
                .retain_runnable(&query.actor, context, commands, &user_steps)
                .await?;
        }
        commands.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(commands)
    }

    async fn get_command(
        &self,
        query: GetCommandQuery,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        self.resolve_command_definition(query.actor, &query.command_id)
            .await
    }

    async fn execute_command(&self, cmd: ExecuteCommandCmd) -> ServiceResult<CommandResultDto> {
        Self::ensure_call_within_bounds(&cmd)?;

        // Snapshot the audit fields BEFORE moving `cmd` into
        // `execute_command_inner` (which consumes args/context).
        let actor_id = cmd.actor.user_id;
        let unparsed_pipe =
            cmd.is_pipe && pipe_parser::parse_pipe_expression(&cmd.command_id).is_err();
        let command_id_for_audit = if cmd.is_pipe {
            pipe_parser::audit_expression(&cmd.command_id)
        } else {
            cmd.command_id.clone()
        };
        let initiator_for_audit = cmd.initiator;
        let run_id_for_audit = cmd
            .run_id
            .clone()
            .filter(|_| cmd.initiator == CommandInitiator::Agent);
        let server_id_for_audit: uuid::Uuid = cmd
            .context
            .as_ref()
            .and_then(|c| c.workspace_id)
            .map(|s| s.into())
            .unwrap_or_else(uuid::Uuid::nil);
        let sanitized_args = bound_audit_args(sanitize_args_for_audit(
            cmd.args.as_ref().unwrap_or(&serde_json::Value::Null),
        ));

        let mut resolver = CommandResolver::new(self, cmd.actor);
        let dispatch = Dispatch::resolve(&cmd, &mut resolver).await;
        let (sensitivity, dangerous) = dispatch.as_ref().map_or(
            (CommandSensitivityDto::Safe, false),
            Dispatch::audit_profile,
        );
        let typed_verb = match &dispatch {
            Ok(Dispatch::Single(Some(resolved))) => Some(resolved.definition.typed_verb),
            _ => None,
        };
        let confirmation_kind = match &cmd.confirmation {
            None => "none",
            Some(ConfirmationProof::InBandToken { .. }) => "in_band_token",
        };

        let result = self.execute_command_inner(cmd, dispatch, resolver).await;

        let (outcome, error_kind_str) = match &result {
            Ok(result_dto) => match result_dto.status {
                CommandResultStatusDto::Success => (AuditOutcome::Success, None),
                CommandResultStatusDto::ConfirmationRequired => {
                    (AuditOutcome::ConfirmationRequired, None)
                }
                CommandResultStatusDto::Error => (AuditOutcome::Error, None),
            },
            Err(ServiceError::PermissionDenied | ServiceError::CapabilityMissing { .. }) => (
                AuditOutcome::PermissionDenied,
                Some("PermissionDenied".to_string()),
            ),
            Err(ServiceError::Forbidden(_)) => (
                AuditOutcome::PermissionDenied,
                Some("Forbidden".to_string()),
            ),
            Err(ServiceError::RateLimited { .. }) => {
                (AuditOutcome::RateLimited, Some("RateLimited".to_string()))
            }
            Err(err) => (AuditOutcome::Error, Some(audited_error(err, unparsed_pipe))),
        };

        match &result {
            Ok(result_dto) => {
                tracing::info!(
                    target: "audit",
                    event = "command_executed",
                    actor_id = %actor_id,
                    command_id = %command_id_for_audit,
                    initiator = %initiator_for_audit,
                    args = %sanitized_args,
                    result_status = %result_dto.status,
                );
            }
            Err(err) => {
                tracing::info!(
                    target: "audit",
                    event = "command_executed",
                    actor_id = %actor_id,
                    command_id = %command_id_for_audit,
                    initiator = %initiator_for_audit,
                    args = %sanitized_args,
                    result_status = AuditOutcome::Error.as_str(),
                    error = %audited_error(err, unparsed_pipe),
                );
            }
        }

        if let Some(audit_svc) = &self.services.audit_log_service {
            let entry = AuditLogEntryDto {
                id: uuid::Uuid::now_v7(),
                actor_user_id: actor_id.into(),
                server_id: server_id_for_audit,
                command_id: command_id_for_audit,
                initiator: initiator_for_audit.to_string(),
                args_redacted: sanitized_args,
                sensitivity: sensitivity.as_str().to_string(),
                dangerous,
                confirmation_kind: confirmation_kind.to_string(),
                typed_verb,
                outcome,
                error_kind: error_kind_str,
                capability_check_outcome: None,
                confirmation_outcome: None,
                parent_audit_id: None,
                run_id: run_id_for_audit,
                created_at: chrono::Utc::now(),
            };
            if let Err(e) = audit_svc.write_entry(entry).await {
                tracing::warn!(
                    event = "audit_db_write_failed",
                    error = %e,
                    "audit DB write failed (tracing line already emitted)"
                );
            }
        }

        result.map_err(|err| gates::with_retry_safety(err, sensitivity, dangerous))
    }

    async fn create_user_command(
        &self,
        cmd: CreateUserCommandCmd,
    ) -> ServiceResult<UserCommandDto> {
        let key = cmd.command_key.trim();
        if !key.starts_with("user.") {
            return Err(ServiceError::Validation(
                "user command ids must start with `user.`".to_string(),
            ));
        }
        ensure_command_id_size(key, "a user command id")?;
        if !user_commands::is_valid_user_command_key(key) {
            return Err(ServiceError::Validation(
                "a user command id may use only lowercase letters, numbers, dots, dashes or underscores after `user.`".to_string(),
            ));
        }
        Self::validate_user_command_text(&cmd.title, &cmd.description)?;
        let (nl_description, intent_samples) =
            Self::sanitized_agent_text(&cmd.nl_description, &cmd.intent_samples)?;
        Self::validate_user_command_steps(key, &cmd.steps, None)?;
        if self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, key)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "user command already exists".to_string(),
            ));
        }
        let agent_runnable = self
            .validate_user_command_for_actor(cmd.actor, key, &cmd.steps, true)
            .await?;
        if cmd.ai_callable && !agent_runnable {
            return Err(ServiceError::Validation(
                Self::AGENT_RUNNABLE_RULE.to_string(),
            ));
        }

        let created = self
            .repos
            .user_command_repo
            .create_user_command(NewUserCommand {
                owner_user_id: cmd.actor.user_id,
                command_key: key.to_string(),
                title: cmd.title.trim().to_string(),
                description: cmd.description.trim().to_string(),
                steps_json: persisted_steps(&cmd.steps)?,
                is_enabled: true,
                ai_callable: cmd.ai_callable,
                nl_description,
                intent_samples,
            })
            .await?;

        Self::parse_user_command(created)
    }

    async fn update_user_command(
        &self,
        cmd: UpdateUserCommandCmd,
    ) -> ServiceResult<UserCommandDto> {
        let key = cmd.command_id.trim();
        user_commands::validate_user_command_step_shape(
            key,
            &steps_with_stored_asks(cmd.steps.clone(), &[]),
        )?;
        let existing = self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, key)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let stored = Self::parse_user_command(existing.clone())?;
        let steps = steps_with_stored_asks(cmd.steps, &stored.steps);
        Self::validate_user_command_steps(key, &steps, Some(&stored))?;

        Self::validate_user_command_text(&cmd.title, &cmd.description)?;
        let (nl_description, intent_samples) = Self::sanitized_agent_text(
            cmd.nl_description
                .as_deref()
                .unwrap_or(&stored.nl_description),
            cmd.intent_samples
                .as_deref()
                .unwrap_or(&stored.intent_samples),
        )?;
        let agent_runnable = self
            .validate_user_command_for_actor(cmd.actor, key, &steps, false)
            .await?;
        let ai_callable = match cmd.ai_callable {
            Some(true) if !agent_runnable => {
                return Err(ServiceError::Validation(
                    Self::AGENT_RUNNABLE_RULE.to_string(),
                ));
            }
            Some(ai_callable) => ai_callable,
            None => stored.ai_callable && agent_runnable,
        };

        let updated = self
            .repos
            .user_command_repo
            .update_user_command(
                existing.id,
                UpdateUserCommand {
                    title: cmd.title.trim().to_string(),
                    description: cmd.description.trim().to_string(),
                    steps_json: persisted_steps(&steps)?,
                    is_enabled: cmd.is_enabled,
                    ai_callable,
                    nl_description,
                    intent_samples,
                },
            )
            .await?;

        Self::parse_user_command(updated)
    }

    async fn delete_user_command(&self, cmd: DeleteUserCommandCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, &cmd.command_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.repos
            .user_command_repo
            .delete_user_command(existing.id)
            .await
            .map_err(ServiceError::from)
    }

    async fn get_user_command(
        &self,
        query: GetUserCommandQuery,
    ) -> ServiceResult<Option<UserCommandDto>> {
        let key = query.command_id.trim();
        if !user_commands::user_command_exists_locally(key) {
            return Ok(None);
        }
        self.repos
            .user_command_repo
            .get_by_key_for_user(query.actor.user_id, key)
            .await?
            .map(Self::parse_user_command)
            .transpose()
    }

    async fn list_user_commands(
        &self,
        query: ListUserCommandsQuery,
    ) -> ServiceResult<Vec<UserCommandDto>> {
        let mut commands = self
            .repos
            .user_command_repo
            .list_for_user(query.actor.user_id)
            .await?
            .into_iter()
            .map(Self::parse_user_command)
            .collect::<ServiceResult<Vec<_>>>()?;
        commands.sort_by(|left, right| left.command_key.cmp(&right.command_key));
        Ok(commands)
    }

    async fn list_macro_step_commands(
        &self,
        _query: ListMacroStepCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        let mut commands = Self::builtin_commands();
        commands.extend(
            registry::APP_COMMANDS
                .iter()
                .filter(|command| !registry::is_agent_management_command(command.id))
                .map(registry::AppCommand::definition),
        );
        commands.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(commands)
    }

    async fn command_discover(
        &self,
        actor: &Actor,
        intent_phrase: Option<String>,
        limit: Option<u32>,
        workspace_id: Option<rim_core::server::model::ServerId>,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        self.command_discover_inner(actor, intent_phrase, limit, workspace_id)
            .await
    }
}
