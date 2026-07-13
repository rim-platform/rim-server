mod discover;
mod dispatch;
mod executor;
mod gates;
mod pipe_parser;
mod rate_limiter;
pub(crate) mod registry;
mod surface;
mod user_commands;

// Re-export the F1 audit-emission entrypoint so `rim_server::app` can wire
// it without exposing the rest of the registry internals.
pub use registry::emit_ai_callable_inventory_audit;

use async_trait::async_trait;

use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand};
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::confirmation::{
    ConfirmationProof, ConfirmationStore, shared_in_memory_store,
};
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandResultDto, CommandResultStatusDto,
    CommandSurfaceDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    ListCommandsQuery, UpdateUserCommandCmd,
};
use rim_services::command::sensitivity::{CommandSensitivityDto, sanitize_args_for_audit};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::incident::service::IncidentService;
use rim_services::macro_def::service::MacroService;
use rim_services::message::service::MessageService;
use rim_services::oncall::service::OnCallService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::CommandRepos;
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
    /// C1b: `incident.ack`/`incident.escalate` dispatch to the same `IncidentService` methods
    /// as their GraphQL mutations (unified dispatch, no duplicated logic — §10.1).
    pub incident_service: std::sync::Arc<dyn IncidentService>,
    /// C1b: `oncall.who`/`oncall.take_shift` dispatch to the same `OnCallService` methods as
    /// their GraphQL query/mutation (§10.1).
    pub oncall_service: std::sync::Arc<dyn OnCallService>,
    /// Dual-path audit write (Widow §7.5 N3). `None` in test/unit contexts
    /// that don't wire up a DB. When `Some`, every `execute_command` result
    /// writes a structured row alongside the `tracing::info!(target="audit")`
    /// line. Both sinks are kept — the tracing line is for low-cardinality
    /// monitoring (grep/tail); the DB row is for queryable forensics.
    pub audit_log_service: Option<std::sync::Arc<dyn AuditLogService>>,
}

#[derive(Clone)]
pub struct DefaultCommandService {
    pub(super) repos: CommandRepos,
    pub(super) services: CommandServices,
    /// Holds the issued confirmation tokens for dangerous commands (H1
    /// closure, §3.5). Phase 1 ships an in-memory store; Phase 5+ may
    /// swap this to Redis behind the same trait without touching call
    /// sites. Single-node-only is a known Phase 1 limitation (Fury §5).
    pub(super) confirmation_store: std::sync::Arc<dyn ConfirmationStore>,
    /// In-memory per-(actor, command_id) sliding-window rate limiter.
    /// Single-node only in Phase 5; multi-node coordination deferred.
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
    /// Extra command definitions injected by tests. Empty in production.
    #[cfg(test)]
    pub(super) extra_builtins: Vec<CommandDefinitionDto>,
}

impl DefaultCommandService {
    pub(super) const MAX_RECURSION_DEPTH: usize = 8;

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
            #[cfg(test)]
            extra_builtins: vec![],
        }
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
            extra_builtins,
        }
    }

    pub(super) fn builtin_commands() -> Vec<CommandDefinitionDto> {
        registry::builtin_commands()
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
            .await?;

        for user_command in user_commands {
            let dto = Self::parse_user_command(user_command)?;
            if dto.is_enabled {
                commands.push(
                    self.derive_user_command_definition(
                        query.actor,
                        dto,
                        &mut std::collections::BTreeSet::new(),
                    )
                    .await?,
                );
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
                .filter_ai_surface(&query.actor, context, commands)
                .await?;
        } else {
            commands.retain(|definition| {
                Self::command_matches_surface(definition, query.surface, context)
            });
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
        // P4-A1 / P5-A1: snapshot audit fields BEFORE moving `cmd` into
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
        let sanitized_args =
            sanitize_args_for_audit(cmd.args.as_ref().unwrap_or(&serde_json::Value::Null));

        let def_snapshot = self
            .resolve_command_definition(cmd.actor, &cmd.command_id)
            .await
            .ok()
            .flatten();
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
            Some(ConfirmationProof::OutOfBand { .. }) => "out_of_band",
        };

        let result = self.execute_command_inner(cmd).await;

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
        if cmd.title.trim().is_empty() {
            return Err(ServiceError::Validation(
                "user command title must be non-empty".to_string(),
            ));
        }
        if cmd.steps.is_empty() {
            return Err(ServiceError::Validation(
                "user command must have at least one step".to_string(),
            ));
        }
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
        Self::validate_user_command_steps(key, &cmd.steps)?;
        self.validate_user_command_steps_for_actor(
            cmd.actor,
            key,
            &cmd.steps,
            &mut std::collections::BTreeSet::new(),
        )
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
        let existing = self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, key)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if cmd.title.trim().is_empty() {
            return Err(ServiceError::Validation(
                "user command title must be non-empty".to_string(),
            ));
        }
        Self::validate_user_command_steps(key, &cmd.steps)?;
        self.validate_user_command_steps_for_actor(
            cmd.actor,
            key,
            &cmd.steps,
            &mut std::collections::BTreeSet::new(),
        )
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
