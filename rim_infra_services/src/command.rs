mod builtin_executor;
pub mod pipe_parser;
mod registry;
mod user_commands;

use async_recursion::async_recursion;
use async_trait::async_trait;
use serde_json::Value;

use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand};
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto, CommandResultDto,
    CommandResultStatusDto, CommandSurfaceDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    ListCommandsQuery, UpdateUserCommandCmd,
};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::message::service::MessageService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::CommandRepos;
use crate::command::user_commands::PersistedUserCommandStep;

#[derive(Clone)]
pub struct CommandServices {
    pub auth_service: std::sync::Arc<dyn rim_services::auth::service::AuthService>,
    pub conversation_service: std::sync::Arc<dyn ConversationService>,
    pub message_service: std::sync::Arc<dyn MessageService>,
    pub read_state_service: std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>,
    pub workspace_state_service:
        std::sync::Arc<dyn rim_services::workspace_state::service::WorkspaceStateService>,
}

#[derive(Clone)]
pub struct DefaultCommandService {
    repos: CommandRepos,
    services: CommandServices,
}

impl DefaultCommandService {
    const MAX_RECURSION_DEPTH: usize = 8;

    pub fn new(repos: CommandRepos, services: CommandServices) -> Self {
        Self { repos, services }
    }

    fn builtin_commands() -> Vec<CommandDefinitionDto> {
        registry::builtin_commands()
    }

    fn validate_user_command_steps(
        command_key: &str,
        steps: &[UserCommandStepDto],
    ) -> ServiceResult<()> {
        user_commands::validate_user_command_steps(command_key, steps, Self::builtin_definition)
    }

    fn command_matches_surface(
        definition: &CommandDefinitionDto,
        surface: CommandSurfaceDto,
        context: Option<&CommandContextDto>,
    ) -> bool {
        registry::command_matches_surface(definition, surface, context)
    }

    fn parse_user_command(user_command: UserCommand) -> ServiceResult<UserCommandDto> {
        user_commands::parse_user_command(user_command)
    }

    #[async_recursion]
    async fn derive_user_command_definition(
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

        for step in &user_command.steps {
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

            required_context.extend(definition.required_context);
            required_capabilities.extend(definition.required_capabilities);
            keybind_capable &= definition.keybind_capable;
            dangerous |= definition.dangerous;
        }

        seen.remove(&user_command.command_key);

        Ok(user_command.as_command_definition_with_metadata(
            CommandExecutionTargetDto::Backend,
            required_context.into_iter().collect(),
            required_capabilities.into_iter().collect(),
            keybind_capable,
            dangerous,
        ))
    }

    #[async_recursion]
    async fn get_user_command_definition(
        &self,
        actor: Actor,
        command_id: &str,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        let user_command = self
            .repos
            .user_command_repo
            .get_by_key_for_user(actor.user_id, command_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let Some(user_command) = user_command else {
            return Ok(None);
        };
        let dto = Self::parse_user_command(user_command)?;
        if !dto.is_enabled {
            return Ok(None);
        }

        Ok(Some(
            self.derive_user_command_definition(actor, dto, &mut std::collections::BTreeSet::new())
                .await?,
        ))
    }

    #[async_recursion]
    async fn resolve_command_definition(
        &self,
        actor: Actor,
        command_id: &str,
    ) -> ServiceResult<Option<CommandDefinitionDto>> {
        if let Some(definition) = Self::builtin_definition(command_id) {
            return Ok(Some(definition));
        }

        self.get_user_command_definition(actor, command_id).await
    }

    #[async_recursion]
    async fn validate_user_command_steps_for_actor(
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
                    .await
                    .map_err(rim_services::map_repo_error)?
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

    async fn execute_builtin(
        &self,
        actor: Actor,
        command_id: &str,
        args: Option<Value>,
        context: Option<CommandContextDto>,
    ) -> ServiceResult<CommandResultDto> {
        builtin_executor::execute_builtin_command(&self.services, actor, command_id, args, context)
            .await
    }

    #[async_recursion]
    async fn execute_user_command_recursive(
        &self,
        actor: Actor,
        user_command: UserCommandDto,
        context: Option<CommandContextDto>,
        confirmed_dangerous: bool,
        depth: usize,
    ) -> ServiceResult<CommandResultDto> {
        if depth >= Self::MAX_RECURSION_DEPTH {
            return Err(ServiceError::Validation(
                "user command recursion depth exceeded".to_string(),
            ));
        }

        let mut payload = serde_json::Map::new();
        for step in user_command.steps {
            let result = self
                .execute_resolved_command_recursive(
                    actor,
                    &step.command_id,
                    step.args,
                    context.clone(),
                    confirmed_dangerous,
                    depth + 1,
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
        })
    }

    async fn execute_piped(
        &self,
        actor: Actor,
        pipe_expression: &str,
        context: Option<CommandContextDto>,
        confirmed_dangerous: bool,
    ) -> ServiceResult<CommandResultDto> {
        use rim_services::command::dto::PipeStageResultDto;

        let segments = pipe_parser::parse_pipe_expression(pipe_expression)?;

        // ── Pre-validation: resolve all command definitions upfront ──
        //
        // SECURITY: Pre-validation currently checks `dangerous` and `execution_target` only.
        // `required_capabilities` are NOT validated here because `CapabilityService` only
        // exposes `list_capabilities` — there is no `check_capability(actor, cap)` method.
        // Individual builtin commands enforce their own capability requirements internally
        // during execution. Phase 2 should introduce a centralized `has_capability` check
        // on `CapabilityService` and wire it into this loop so that all required capabilities
        // across all pipe stages are validated before any stage executes.
        let mut definitions = Vec::with_capacity(segments.len());
        let mut dangerous_stages: Vec<(usize, String)> = Vec::new();

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

            // Task 6: Reject desktop-local commands in pipes
            if definition.execution_target == CommandExecutionTargetDto::DesktopLocal {
                return Err(ServiceError::Validation(format!(
                    "piped commands must all target the same execution runtime; \
                     stage {} ('{}') is desktop-local",
                    i + 1,
                    segment.command_id,
                )));
            }

            if definition.dangerous {
                dangerous_stages.push((i + 1, segment.command_id.clone()));
            }

            definitions.push(definition);
        }

        // Task 5: Collect all dangerous stages and reject if not confirmed
        if !dangerous_stages.is_empty() && !confirmed_dangerous {
            let stage_list: Vec<String> = dangerous_stages
                .iter()
                .map(|(idx, cmd)| format!("stage {} ('{}')", idx, cmd))
                .collect();
            return Err(ServiceError::Validation(format!(
                "dangerous command requires confirmation — dangerous stages: {}",
                stage_list.join(", "),
            )));
        }

        // ── Execute pipeline stages ──
        let mut previous_payload: Option<serde_json::Value> = None;
        let mut all_results = serde_json::Map::new();
        let mut pipeline_stages: Vec<PipeStageResultDto> = Vec::with_capacity(segments.len());

        for (i, segment) in segments.iter().enumerate() {
            let merged_args = merge_pipe_args(previous_payload.as_ref(), segment.args.as_ref());

            let prev_command_id = if i > 0 {
                Some(segments[i - 1].command_id.as_str())
            } else {
                None
            };

            let result = self
                .execute_resolved_command_recursive(
                    actor,
                    &segment.command_id,
                    merged_args,
                    context.clone(),
                    confirmed_dangerous,
                    0,
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
                    // Task 7: Enhanced error messages for piped context
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

                    // Task 7: Return partial results for completed stages
                    return Ok(CommandResultDto {
                        status: CommandResultStatusDto::Error,
                        message: error_message,
                        payload: Some(Value::Object(all_results)),
                        follow_up_commands: vec![],
                        pipeline_stages: Some(pipeline_stages),
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
        })
    }

    fn builtin_definition(command_id: &str) -> Option<CommandDefinitionDto> {
        Self::builtin_commands()
            .into_iter()
            .find(|definition| definition.id == command_id)
    }

    #[async_recursion]
    async fn execute_resolved_command_recursive(
        &self,
        actor: Actor,
        command_id: &str,
        args: Option<Value>,
        context: Option<CommandContextDto>,
        confirmed_dangerous: bool,
        depth: usize,
    ) -> ServiceResult<CommandResultDto> {
        if let Some(definition) = Self::builtin_definition(command_id) {
            if definition.dangerous && !confirmed_dangerous {
                return Err(ServiceError::Validation(
                    "dangerous command requires confirmation".to_string(),
                ));
            }
            return self.execute_builtin(actor, command_id, args, context).await;
        }

        let user_command = self
            .repos
            .user_command_repo
            .get_by_key_for_user(actor.user_id, command_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .map(Self::parse_user_command)
            .transpose()?
            .ok_or(ServiceError::NotFound)?;

        self.execute_user_command_recursive(
            actor,
            user_command,
            context,
            confirmed_dangerous,
            depth,
        )
        .await
    }
}

// SECURITY: All keys from the previous stage's output pass through to the next stage
// without filtering. A crafted first stage could inject unexpected keys into downstream
// commands. This is acceptable for Phase 1 because builtin commands extract only the
// args they declare and ignore unknown keys. Phase 2 should introduce whitelist filtering
// based on the receiving command's declared arguments so that only expected keys propagate.
fn merge_pipe_args(
    piped: Option<&serde_json::Value>,
    explicit: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    match (piped, explicit) {
        (None, None) => None,
        (Some(p), None) => Some(p.clone()),
        (None, Some(e)) => Some(e.clone()),
        (Some(p), Some(e)) => {
            let mut merged = match p {
                serde_json::Value::Object(m) => m.clone(),
                _ => serde_json::Map::new(),
            };
            if let serde_json::Value::Object(explicit_map) = e {
                for (k, v) in explicit_map {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Some(serde_json::Value::Object(merged))
        }
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
            .await
            .map_err(rim_services::map_repo_error)?;

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

        commands
            .retain(|definition| Self::command_matches_surface(definition, query.surface, context));
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
        if cmd.is_pipe {
            return self
                .execute_piped(
                    cmd.actor,
                    &cmd.command_id,
                    cmd.context,
                    cmd.confirmed_dangerous,
                )
                .await;
        }
        if cmd.command_id.contains('|') {
            return Err(ServiceError::Validation(
                "command_id contains pipe character '|' but is_pipe was not set to true".into(),
            ));
        }

        self.execute_resolved_command_recursive(
            cmd.actor,
            &cmd.command_id,
            cmd.args,
            cmd.context,
            cmd.confirmed_dangerous,
            0,
        )
        .await
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
            .await
            .map_err(rim_services::map_repo_error)?
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
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?
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
            .await
            .map_err(rim_services::map_repo_error)?;

        Self::parse_user_command(updated)
    }

    async fn delete_user_command(&self, cmd: DeleteUserCommandCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .user_command_repo
            .get_by_key_for_user(cmd.actor.user_id, &cmd.command_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        self.repos
            .user_command_repo
            .delete_user_command(existing.id)
            .await
            .map_err(rim_services::map_repo_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rim_core::command::model::{NewUserCommand, UserCommandId};
    use rim_core::command::repository::UserCommandRepository;
    use rim_core::repository::error::RepoResult;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use std::sync::Arc;

    mockall::mock! {
        UserCommandRepository {}
        #[async_trait]
        impl UserCommandRepository for UserCommandRepository {
            async fn get_by_id(&self, id: UserCommandId) -> RepoResult<Option<UserCommand>>;
            async fn get_by_key_for_user(
                &self,
                owner_user_id: rim_core::user::model::UserId,
                command_key: &str,
            ) -> RepoResult<Option<UserCommand>>;
            async fn list_for_user(
                &self,
                owner_user_id: rim_core::user::model::UserId,
            ) -> RepoResult<Vec<UserCommand>>;
            async fn create_user_command(
                &self,
                new_command: NewUserCommand,
            ) -> RepoResult<UserCommand>;
            async fn update_user_command(
                &self,
                id: UserCommandId,
                update: UpdateUserCommand,
            ) -> RepoResult<UserCommand>;
            async fn delete_user_command(&self, id: UserCommandId) -> RepoResult<()>;
        }
    }

    fn service(
        repo: MockUserCommandRepository,
        conversation_service: MockConversationService,
        message_service: MockMessageService,
    ) -> DefaultCommandService {
        DefaultCommandService::new(
            CommandRepos {
                user_command_repo: Arc::new(repo),
            },
            CommandServices {
                auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
                conversation_service: Arc::new(conversation_service),
                message_service: Arc::new(message_service),
                read_state_service: Arc::new(
                    rim_services::read_state::service::MockReadStateService::new(),
                ),
                workspace_state_service: Arc::new(
                    rim_services::workspace_state::service::MockWorkspaceStateService::new(),
                ),
            },
        )
    }

    #[tokio::test]
    async fn list_commands_includes_builtins_and_user_commands() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().return_once(move |owner_user_id| {
            assert_eq!(owner_user_id, actor_user_id);
            Ok(vec![UserCommand {
                id: rim_core::command::model::UserCommandId::new(),
                owner_user_id,
                command_key: "user.test".to_string(),
                title: "Test".to_string(),
                description: "desc".to_string(),
                steps_json: serde_json::json!([{ "command_id": "chat.post_message", "args": null }]),
                is_enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }])
        });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let commands = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .unwrap();

        assert!(
            commands
                .iter()
                .any(|command| command.id == "chat.post_message")
        );
        assert!(commands.iter().any(|command| command.id == "user.test"));
    }

    #[tokio::test]
    async fn create_user_command_validates_prefix() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let repo = MockUserCommandRepository::new();
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .create_user_command(CreateUserCommandCmd {
                actor,
                command_key: "bad.id".to_string(),
                title: "Bad".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn execute_builtin_backend_command_posts_message() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let repo = MockUserCommandRepository::new();
        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| {
                assert_eq!(cmd.actor.user_id, actor_user_id);
                assert_eq!(cmd.text.as_deref(), Some("hello"));
                Box::pin(async move {
                    Ok(MessageDto {
                        id: rim_core::message::model::MessageId::new(),
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        thread_id: None,
                        root_thread_id: None,
                        author_id: cmd.actor.user_id,
                        text: cmd.text,
                        has_attachments: false,
                        attachments: vec![],
                        parent_message_id: None,
                        forwarded_from_message_id: None,
                        thread_root_message_id: None,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                    })
                })
            });
        let svc = service(repo, MockConversationService::new(), message_service);

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "chat.post_message".to_string(),
                args: Some(serde_json::json!({
                    "server_id": rim_core::server::model::ServerId::new().to_string(),
                    "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                    "text": "hello"
                })),
                context: None,
                confirmed_dangerous: false,
                is_pipe: false,
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn list_commands_filters_context_menu_by_required_context() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let without_context = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::ContextMenu,
            })
            .await
            .unwrap();
        assert!(without_context.is_empty());

        let with_context = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(CommandContextDto {
                    workspace_id: Some(rim_core::server::model::ServerId::new()),
                    conversation_id: Some(rim_core::conversation::model::ConversationId::new()),
                    thread_id: None,
                    selected_text: None,
                    selected_message_id: Some(rim_core::message::model::MessageId::new()),
                    selected_message_ids: vec![],
                    focused_surface: Some("message_list".to_string()),
                    active_panel: None,
                }),
                surface: CommandSurfaceDto::ContextMenu,
            })
            .await
            .unwrap();
        assert!(
            with_context
                .iter()
                .any(|command| command.id == "chat.reply_to_selected_message")
        );
    }

    #[tokio::test]
    async fn create_user_command_rejects_self_reference() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user().returning(|_, _| Ok(None));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .create_user_command(CreateUserCommandCmd {
                actor,
                command_key: "user.loop".to_string(),
                title: "Loop".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "user.loop".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn update_and_delete_user_command_work() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let existing = UserCommand {
            id: rim_core::command::model::UserCommandId::new(),
            owner_user_id: actor.user_id,
            command_key: "user.quick_group".to_string(),
            title: "Quick Group".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!([{ "command_id": "chat.create_group", "args": null }]),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let updated = UserCommand {
            title: "Quick Group Updated".to_string(),
            is_enabled: false,
            ..existing.clone()
        };
        let mut repo = MockUserCommandRepository::new();
        let existing_for_lookup = existing.clone();
        let updated_for_return = updated.clone();
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, command_key| {
                assert_eq!(owner_user_id, actor_user_id);
                assert_eq!(command_key, existing_for_lookup.command_key.as_str());
                Ok(Some(existing_for_lookup))
            });
        repo.expect_update_user_command()
            .return_once(move |id, update_cmd| {
                assert_eq!(id, updated_for_return.id);
                assert_eq!(update_cmd.title, "Quick Group Updated");
                assert_eq!(update_cmd.description, "desc");
                assert!(!update_cmd.is_enabled);
                Ok(updated_for_return)
            });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let result = svc
            .update_user_command(UpdateUserCommandCmd {
                actor,
                command_id: existing.command_key.clone(),
                title: updated.title.clone(),
                description: updated.description.clone(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.create_group".to_string(),
                    args: None,
                }],
                is_enabled: false,
            })
            .await
            .unwrap();
        assert_eq!(result.title, "Quick Group Updated");
        assert!(!result.is_enabled);

        let mut repo = MockUserCommandRepository::new();
        let delete_actor_user_id = actor.user_id;
        let expected_command_key = existing.command_key.clone();
        let expected_command_id = existing.id;
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, command_key| {
                assert_eq!(owner_user_id, delete_actor_user_id);
                assert_eq!(command_key, expected_command_key.as_str());
                Ok(Some(existing))
            });
        repo.expect_delete_user_command().return_once(move |id| {
            assert_eq!(id, expected_command_id);
            Ok(())
        });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );
        svc.delete_user_command(DeleteUserCommandCmd {
            actor,
            command_id: "user.quick_group".to_string(),
        })
        .await
        .unwrap();
    }
}
