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
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    GetUserCommandQuery, ListCommandsQuery, ListUserCommandsQuery, UpdateUserCommandCmd,
};
use rim_services::command::sensitivity::{CommandSensitivityDto, sanitize_args_for_audit};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::macro_def::service::MacroService;
use rim_services::message::service::MessageService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::CommandRepos;
use crate::command::resolver::CommandResolver;
use crate::command::user_commands::PersistedUserCommandStep;

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
    /// AI-surface macro discovery source, wired **after** construction to break
    /// the command↔macro circular init dependency (provider builds the command
    /// service first, then the macro service from it, then calls
    /// [`Self::set_macro_service`]). Wrapped in `Arc<OnceLock<_>>` so that the
    /// single write is visible through every `Arc<dyn CommandService>` clone the
    /// provider hands to keybind/pipeline/macro. `None` (unset) in test contexts
    /// and until the provider wires it. When set, `list_commands(Surface::Ai)`
    /// delegates macro synthesis to `MacroService::synthesize_command_definitions`.
    pub(super) macro_service: std::sync::Arc<std::sync::OnceLock<std::sync::Arc<dyn MacroService>>>,
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
            macro_service: std::sync::Arc::new(std::sync::OnceLock::new()),
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
            macro_service: std::sync::Arc::new(std::sync::OnceLock::new()),
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

    pub(super) fn validate_user_command_steps(
        command_key: &str,
        steps: &[UserCommandStepDto],
    ) -> ServiceResult<()> {
        user_commands::validate_user_command_steps(command_key, steps, Self::builtin_definition)
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

    /// Wire the macro service into this command service exactly once, after the
    /// macro service has been constructed. Idempotent-safe: a second call is a
    /// no-op and is logged, never a panic. Returns `Ok(())` on the first set,
    /// `Err(())` if already set — callers may ignore the result in production
    /// wiring where set-once is guaranteed by construction order.
    // `Err(())` carries no information beyond "already set" (see the
    // tracing::warn! above for the diagnostic); a dedicated error type would
    // add ceremony without adding value for a set-once cell.
    #[allow(clippy::result_unit_err)]
    pub(crate) fn set_macro_service(
        &self,
        macro_service: std::sync::Arc<dyn MacroService>,
    ) -> Result<(), ()> {
        self.macro_service.set(macro_service).map_err(|_| {
            tracing::warn!(
                event = "command_service_macro_double_set",
                "set_macro_service called after macro service already wired; ignoring"
            );
        })
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
            // Delegate AI-callable macro discovery to MacroService — single
            // source of truth for MacroDef→CommandDefinitionDto mapping.
            if let (Some(macro_svc), Some(server_id)) = (
                self.macro_service.get().map(std::sync::Arc::as_ref),
                context.and_then(|c| c.workspace_id),
            ) {
                use rim_services::macro_def::request::SynthesizeMacroCommandsQuery;
                let synthesized = macro_svc
                    .synthesize_command_definitions(SynthesizeMacroCommandsQuery {
                        actor: query.actor,
                        server_id,
                    })
                    .await?;
                commands.extend(synthesized);
            }

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
        let command_id_for_audit = cmd.command_id.clone();
        let initiator_for_audit = cmd.initiator;
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
        let top_level = if cmd.is_pipe {
            Ok(None)
        } else {
            resolver.resolve(&cmd.command_id).await
        };
        let def_snapshot = top_level
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|resolved| resolved.definition.clone());
        let sensitivity_str = def_snapshot
            .as_ref()
            .map_or("safe", |d| match d.sensitivity {
                CommandSensitivityDto::Safe => "safe",
                CommandSensitivityDto::Sensitive => "sensitive",
                CommandSensitivityDto::Mutating => "mutating",
                CommandSensitivityDto::Restricted => "restricted",
            });
        let dangerous = def_snapshot.as_ref().is_some_and(|d| d.dangerous);
        let confirmation_kind = match &cmd.confirmation {
            None => "none",
            Some(ConfirmationProof::InBandToken { .. }) => "in_band_token",
        };

        let result = self.execute_command_inner(cmd, top_level, resolver).await;

        let (outcome, error_kind_str) = match &result {
            Ok(result_dto) => match result_dto.status {
                CommandResultStatusDto::Success => (AuditOutcome::Success, None),
                CommandResultStatusDto::ConfirmationRequired => {
                    (AuditOutcome::ConfirmationRequired, None)
                }
                CommandResultStatusDto::Error => (AuditOutcome::Error, None),
            },
            Err(ServiceError::PermissionDenied) => (
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
            Err(err) => (AuditOutcome::Error, Some(format!("{err}"))),
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
                    error = %err,
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
                sensitivity: sensitivity_str.to_string(),
                dangerous,
                confirmation_kind: confirmation_kind.to_string(),
                typed_verb: def_snapshot.as_ref().map(|d| d.typed_verb),
                outcome,
                error_kind: error_kind_str,
                capability_check_outcome: None,
                confirmation_outcome: None,
                parent_audit_id: None,
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

        result
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
        Self::validate_user_command_steps(key, &cmd.steps)?;
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
        self.validate_user_command_for_actor(cmd.actor, key, &cmd.steps, true)
            .await?;

        let created = self
            .repos
            .user_command_repo
            .create_user_command(NewUserCommand {
                owner_user_id: cmd.actor.user_id,
                command_key: key.to_string(),
                title: cmd.title.trim().to_string(),
                description: cmd.description.trim().to_string(),
                steps_json: serde_json::to_value(
                    cmd.steps
                        .iter()
                        .map(|step| PersistedUserCommandStep {
                            command_id: step.command_id.clone(),
                            args: step.args.clone(),
                        })
                        .collect::<Vec<_>>(),
                )
                .map_err(ServiceError::internal)?,
                is_enabled: true,
            })
            .await?;

        Self::parse_user_command(created)
    }

    async fn update_user_command(
        &self,
        cmd: UpdateUserCommandCmd,
    ) -> ServiceResult<UserCommandDto> {
        let key = cmd.command_id.trim();
        Self::validate_user_command_steps(key, &cmd.steps)?;
        let existing = self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, key)
            .await?
            .ok_or(ServiceError::NotFound)?;

        Self::validate_user_command_text(&cmd.title, &cmd.description)?;
        self.validate_user_command_for_actor(cmd.actor, key, &cmd.steps, false)
            .await?;

        let updated = self
            .repos
            .user_command_repo
            .update_user_command(
                existing.id,
                UpdateUserCommand {
                    title: cmd.title.trim().to_string(),
                    description: cmd.description.trim().to_string(),
                    steps_json: serde_json::to_value(
                        cmd.steps
                            .iter()
                            .map(|step| PersistedUserCommandStep {
                                command_id: step.command_id.clone(),
                                args: step.args.clone(),
                            })
                            .collect::<Vec<_>>(),
                    )
                    .map_err(ServiceError::internal)?,
                    is_enabled: cmd.is_enabled,
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
