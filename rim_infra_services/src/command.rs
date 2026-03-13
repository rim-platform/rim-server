mod builtin_executor;
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
    pub conversation_service: std::sync::Arc<dyn ConversationService>,
    pub message_service: std::sync::Arc<dyn MessageService>,
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
                    true,
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

        self.execute_user_command_recursive(actor, user_command, context, depth)
            .await
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
    use rim_core::command::model::{NewUserCommand, UserCommandId};
    use rim_core::command::repository::UserCommandRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_services::conversation::command::CreateConversationCmd;
    use rim_services::conversation::dto::ConversationDto;
    use rim_services::message::command::SendMessageCmd;
    use rim_services::message::dto::MessageDto;
    use std::sync::Arc;

    struct FakeUserCommandRepository {
        list_result: std::sync::Mutex<Option<RepoResult<Vec<UserCommand>>>>,
        get_result: std::sync::Mutex<Option<RepoResult<Option<UserCommand>>>>,
        create_result: std::sync::Mutex<Option<RepoResult<UserCommand>>>,
        update_result: std::sync::Mutex<Option<RepoResult<UserCommand>>>,
        delete_result: std::sync::Mutex<Option<RepoResult<()>>>,
    }

    #[async_trait]
    impl UserCommandRepository for FakeUserCommandRepository {
        async fn get_by_id(&self, _id: UserCommandId) -> RepoResult<Option<UserCommand>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_by_key_for_user(
            &self,
            _owner_user_id: rim_core::user::model::UserId,
            _command_key: &str,
        ) -> RepoResult<Option<UserCommand>> {
            self.get_result.lock().unwrap().take().unwrap_or(Ok(None))
        }

        async fn list_for_user(
            &self,
            _owner_user_id: rim_core::user::model::UserId,
        ) -> RepoResult<Vec<UserCommand>> {
            self.list_result
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Ok(vec![]))
        }

        async fn create_user_command(
            &self,
            _new_command: NewUserCommand,
        ) -> RepoResult<UserCommand> {
            self.create_result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(RepoError::new(RepoErrorKind::Unsupported)))
        }

        async fn update_user_command(
            &self,
            _id: UserCommandId,
            _update: UpdateUserCommand,
        ) -> RepoResult<UserCommand> {
            self.update_result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(RepoError::new(RepoErrorKind::Unsupported)))
        }

        async fn delete_user_command(&self, _id: UserCommandId) -> RepoResult<()> {
            self.delete_result.lock().unwrap().take().unwrap_or(Ok(()))
        }
    }

    struct FakeConversationService {
        create_result: std::sync::Mutex<Option<ServiceResult<ConversationDto>>>,
    }

    #[async_trait]
    impl ConversationService for FakeConversationService {
        async fn create_conversation(
            &self,
            _cmd: CreateConversationCmd,
        ) -> ServiceResult<ConversationDto> {
            self.create_result.lock().unwrap().take().unwrap()
        }

        async fn get_conversation(
            &self,
            _query: rim_services::conversation::command::GetConversationQuery,
        ) -> ServiceResult<ConversationDto> {
            unreachable!()
        }
        async fn list_inbox_conversations(
            &self,
            _query: rim_services::conversation::command::ListInboxConversationsQuery,
        ) -> ServiceResult<
            rim_core::repository::pagination::Page<
                ConversationDto,
                rim_services::conversation::dto::InboxPageCursor,
            >,
        > {
            unreachable!()
        }
        async fn list_channels(
            &self,
            _query: rim_services::conversation::command::ListChannelsQuery,
        ) -> ServiceResult<
            rim_core::repository::pagination::Page<
                ConversationDto,
                rim_services::conversation::dto::ChannelPageCursor,
            >,
        > {
            unreachable!()
        }
        async fn add_member(
            &self,
            _cmd: rim_services::conversation::command::AddConversationMemberCmd,
        ) -> ServiceResult<ConversationDto> {
            unreachable!()
        }
        async fn remove_member(
            &self,
            _cmd: rim_services::conversation::command::RemoveConversationMemberCmd,
        ) -> ServiceResult<ConversationDto> {
            unreachable!()
        }
    }

    struct FakeMessageService {
        send_result: std::sync::Mutex<Option<ServiceResult<MessageDto>>>,
    }

    #[async_trait]
    impl MessageService for FakeMessageService {
        async fn send_message(&self, _cmd: SendMessageCmd) -> ServiceResult<MessageDto> {
            self.send_result.lock().unwrap().take().unwrap()
        }

        async fn send_thread_message(
            &self,
            _cmd: rim_services::message::command::SendThreadMessageCmd,
        ) -> ServiceResult<MessageDto> {
            unreachable!()
        }
        async fn get_message(
            &self,
            _query: rim_services::message::command::GetMessageQuery,
        ) -> ServiceResult<MessageDto> {
            unreachable!()
        }
        async fn get_message_reference(
            &self,
            _query: rim_services::message::command::GetMessageReferenceQuery,
        ) -> ServiceResult<rim_services::message::dto::MessageReferenceDto> {
            unreachable!()
        }
        async fn list_messages(
            &self,
            _query: rim_services::message::command::ListMessagesQuery,
        ) -> ServiceResult<
            rim_core::repository::pagination::Page<
                MessageDto,
                rim_services::message::dto::MessagePageCursor,
            >,
        > {
            unreachable!()
        }
        async fn list_thread_messages(
            &self,
            _query: rim_services::message::command::ListThreadMessagesQuery,
        ) -> ServiceResult<
            rim_core::repository::pagination::Page<
                MessageDto,
                rim_services::message::dto::MessagePageCursor,
            >,
        > {
            unreachable!()
        }
        async fn search_messages(
            &self,
            _query: rim_services::message::command::SearchMessagesQuery,
        ) -> ServiceResult<Vec<MessageDto>> {
            unreachable!()
        }
        async fn edit_message(
            &self,
            _cmd: rim_services::message::command::EditMessageCmd,
        ) -> ServiceResult<MessageDto> {
            unreachable!()
        }
        async fn delete_message(
            &self,
            _cmd: rim_services::message::command::DeleteMessageCmd,
        ) -> ServiceResult<()> {
            unreachable!()
        }
    }

    fn service(
        repo: FakeUserCommandRepository,
        conversation_result: ServiceResult<ConversationDto>,
        message_result: ServiceResult<MessageDto>,
    ) -> DefaultCommandService {
        DefaultCommandService::new(
            CommandRepos {
                user_command_repo: Arc::new(repo),
            },
            CommandServices {
                conversation_service: Arc::new(FakeConversationService {
                    create_result: std::sync::Mutex::new(Some(conversation_result)),
                }),
                message_service: Arc::new(FakeMessageService {
                    send_result: std::sync::Mutex::new(Some(message_result)),
                }),
            },
        )
    }

    #[tokio::test]
    async fn list_commands_includes_builtins_and_user_commands() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(Some(Ok(vec![UserCommand {
                id: rim_core::command::model::UserCommandId::new(),
                owner_user_id: actor.user_id,
                command_key: "user.test".to_string(),
                title: "Test".to_string(),
                description: "desc".to_string(),
                steps_json: serde_json::json!([{ "command_id": "chat.post_message", "args": null }]),
                is_enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }]))),
            get_result: std::sync::Mutex::new(None),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(None),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
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
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(None),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(None),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
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
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(None),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(None),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Ok(MessageDto {
                id: rim_core::message::model::MessageId::new(),
                server_id: rim_core::server::model::ServerId::new(),
                conversation_id: rim_core::conversation::model::ConversationId::new(),
                thread_id: None,
                root_thread_id: None,
                author_id: actor.user_id,
                text: Some("hello".to_string()),
                has_attachments: false,
                attachments: vec![],
                parent_message_id: None,
                is_deleted: false,
                sent_at: chrono::Utc::now(),
            }),
        );

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
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn list_commands_filters_context_menu_by_required_context() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(None),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(None),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
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
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(None),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(None),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
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
        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(Some(Ok(Some(existing.clone())))),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(Some(Ok(updated.clone()))),
            delete_result: std::sync::Mutex::new(Some(Ok(()))),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
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

        let repo = FakeUserCommandRepository {
            list_result: std::sync::Mutex::new(None),
            get_result: std::sync::Mutex::new(Some(Ok(Some(existing)))),
            create_result: std::sync::Mutex::new(None),
            update_result: std::sync::Mutex::new(None),
            delete_result: std::sync::Mutex::new(Some(Ok(()))),
        };
        let svc = service(
            repo,
            Err(ServiceError::NotFound),
            Err(ServiceError::NotFound),
        );
        svc.delete_user_command(DeleteUserCommandCmd {
            actor,
            command_id: "user.quick_group".to_string(),
        })
        .await
        .unwrap();
    }
}
