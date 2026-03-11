use async_trait::async_trait;

use rim_core::keybind::model::{Keybind, KeybindSubject, NewKeybind, UpdateKeybind};
use rim_services::command::dto::CommandSurfaceDto;
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::service::CommandService;
use rim_services::keybind::dto::{KeybindDto, KeybindScopeDto};
use rim_services::keybind::request::{
    CreateKeybindCmd, DeleteKeybindCmd, ListKeybindsQuery, UpdateKeybindCmd,
};
use rim_services::keybind::service::KeybindService;
use rim_services::{ServiceError, ServiceResult};

use crate::repos::KeybindRepos;
use crate::util::{ensure_server_member, ensure_server_owner};

#[derive(Clone)]
pub struct KeybindServices {
    pub command_service: std::sync::Arc<dyn CommandService>,
}

#[derive(Clone)]
pub struct DefaultKeybindService {
    repos: KeybindRepos,
    services: KeybindServices,
}

impl DefaultKeybindService {
    pub fn new(repos: KeybindRepos, services: KeybindServices) -> Self {
        Self { repos, services }
    }

    fn subject_for_request(
        actor_user_id: rim_core::user::model::UserId,
        scope: KeybindScopeDto,
        workspace_id: Option<rim_core::server::model::WorkspaceId>,
    ) -> ServiceResult<KeybindSubject> {
        match scope {
            KeybindScopeDto::User => Ok(KeybindSubject::User(actor_user_id)),
            KeybindScopeDto::Workspace => {
                workspace_id.map(KeybindSubject::Workspace).ok_or_else(|| {
                    ServiceError::Validation(
                        "workspace_id is required for workspace keybinds".to_string(),
                    )
                })
            }
        }
    }

    async fn ensure_scope_access(
        &self,
        actor: rim_services::Actor,
        subject: KeybindSubject,
    ) -> ServiceResult<()> {
        match subject {
            KeybindSubject::User(user_id) => {
                if user_id != actor.user_id {
                    return Err(ServiceError::PermissionDenied);
                }
            }
            KeybindSubject::Workspace(workspace_id) => {
                ensure_server_owner(
                    self.repos.server_member_repo.as_ref(),
                    workspace_id,
                    actor.user_id,
                )
                .await?;
            }
        }
        Ok(())
    }

    fn to_dto(keybind: Keybind) -> KeybindDto {
        let (scope, workspace_id) = match keybind.subject {
            KeybindSubject::User(_) => (KeybindScopeDto::User, None),
            KeybindSubject::Workspace(workspace_id) => {
                (KeybindScopeDto::Workspace, Some(workspace_id))
            }
        };

        KeybindDto {
            id: keybind.id,
            scope,
            workspace_id,
            keystroke: keybind.keystroke,
            command_id: keybind.command_id,
            context: keybind.context.into(),
            preset_args: keybind.preset_args,
            is_enabled: keybind.is_enabled,
        }
    }

    async fn validate_command_bindable(
        &self,
        actor: rim_services::Actor,
        command_id: &str,
    ) -> ServiceResult<()> {
        let commands = self
            .services
            .command_service
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::KeybindPicker,
            })
            .await?;

        if commands.iter().any(|command| command.id == command_id) {
            Ok(())
        } else {
            Err(ServiceError::Validation(format!(
                "command `{command_id}` is not keybind-capable"
            )))
        }
    }
}

#[async_trait]
impl KeybindService for DefaultKeybindService {
    async fn list_keybinds(&self, query: ListKeybindsQuery) -> ServiceResult<Vec<KeybindDto>> {
        let subject =
            Self::subject_for_request(query.actor.user_id, query.scope, query.workspace_id)?;
        match subject {
            KeybindSubject::User(user_id) => {
                if user_id != query.actor.user_id {
                    return Err(ServiceError::PermissionDenied);
                }
            }
            KeybindSubject::Workspace(workspace_id) => {
                ensure_server_member(
                    self.repos.server_member_repo.as_ref(),
                    workspace_id,
                    query.actor.user_id,
                )
                .await?;
            }
        }

        let mut keybinds = self
            .repos
            .keybind_repo
            .list_for_subject(subject)
            .await
            .map_err(rim_services::map_repo_error)?;
        keybinds.sort_by(|left, right| {
            left.keystroke
                .cmp(&right.keystroke)
                .then(left.command_id.cmp(&right.command_id))
        });
        Ok(keybinds.into_iter().map(Self::to_dto).collect())
    }

    async fn create_keybind(&self, cmd: CreateKeybindCmd) -> ServiceResult<KeybindDto> {
        let subject = Self::subject_for_request(cmd.actor.user_id, cmd.scope, cmd.workspace_id)?;
        self.ensure_scope_access(cmd.actor, subject).await?;
        self.validate_command_bindable(cmd.actor, &cmd.command_id)
            .await?;
        if cmd.keystroke.trim().is_empty() {
            return Err(ServiceError::Validation(
                "keystroke must be non-empty".to_string(),
            ));
        }

        if self
            .repos
            .keybind_repo
            .get_by_binding(
                cmd.scope.into(),
                subject,
                cmd.context.into(),
                cmd.keystroke.trim(),
            )
            .await
            .map_err(rim_services::map_repo_error)?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "keybind already exists for this scope/context/keystroke".to_string(),
            ));
        }

        let created = self
            .repos
            .keybind_repo
            .create_keybind(NewKeybind {
                scope: cmd.scope.into(),
                subject,
                keystroke: cmd.keystroke.trim().to_string(),
                command_id: cmd.command_id,
                context: cmd.context.into(),
                preset_args: cmd.preset_args,
                is_enabled: true,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::to_dto(created))
    }

    async fn update_keybind(&self, cmd: UpdateKeybindCmd) -> ServiceResult<KeybindDto> {
        let existing = self
            .repos
            .keybind_repo
            .get_by_id(cmd.keybind_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;
        self.ensure_scope_access(cmd.actor, existing.subject)
            .await?;
        self.validate_command_bindable(cmd.actor, &cmd.command_id)
            .await?;
        if cmd.keystroke.trim().is_empty() {
            return Err(ServiceError::Validation(
                "keystroke must be non-empty".to_string(),
            ));
        }

        if let Some(other) = self
            .repos
            .keybind_repo
            .get_by_binding(
                existing.scope,
                existing.subject,
                cmd.context.into(),
                cmd.keystroke.trim(),
            )
            .await
            .map_err(rim_services::map_repo_error)?
            && other.id != existing.id
        {
            return Err(ServiceError::Conflict(
                "keybind already exists for this scope/context/keystroke".to_string(),
            ));
        }

        let updated = self
            .repos
            .keybind_repo
            .update_keybind(
                existing.id,
                UpdateKeybind {
                    keystroke: cmd.keystroke.trim().to_string(),
                    command_id: cmd.command_id,
                    context: cmd.context.into(),
                    preset_args: cmd.preset_args,
                    is_enabled: cmd.is_enabled,
                },
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::to_dto(updated))
    }

    async fn delete_keybind(&self, cmd: DeleteKeybindCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .keybind_repo
            .get_by_id(cmd.keybind_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;
        self.ensure_scope_access(cmd.actor, existing.subject)
            .await?;

        self.repos
            .keybind_repo
            .delete_keybind(existing.id)
            .await
            .map_err(rim_services::map_repo_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rim_core::keybind::model::{KeybindContext, KeybindScope};
    use rim_core::keybind::repository::KeybindRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_services::command::dto::{
        CommandDefinitionDto, CommandExecutionTargetDto, CommandSourceKindDto,
    };
    use rim_services::command::dto::{CommandResultDto, UserCommandDto};
    use rim_services::command::request::{
        CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, ListCommandsQuery,
        UpdateUserCommandCmd,
    };
    use rim_services::command::service::CommandService;

    struct FakeKeybindRepository {
        existing: std::sync::Mutex<Option<Keybind>>,
        created: std::sync::Mutex<Option<Keybind>>,
    }

    #[async_trait]
    impl KeybindRepository for FakeKeybindRepository {
        async fn get_by_id(
            &self,
            id: rim_core::keybind::model::KeybindId,
        ) -> RepoResult<Option<Keybind>> {
            Ok(self.existing.lock().unwrap().clone().filter(|k| k.id == id))
        }

        async fn list_for_subject(&self, _subject: KeybindSubject) -> RepoResult<Vec<Keybind>> {
            Ok(self.existing.lock().unwrap().clone().into_iter().collect())
        }

        async fn get_by_binding(
            &self,
            _scope: KeybindScope,
            _subject: KeybindSubject,
            _context: KeybindContext,
            _keystroke: &str,
        ) -> RepoResult<Option<Keybind>> {
            Ok(self.existing.lock().unwrap().clone())
        }

        async fn create_keybind(&self, _new_keybind: NewKeybind) -> RepoResult<Keybind> {
            Ok(self.created.lock().unwrap().clone().unwrap())
        }

        async fn update_keybind(
            &self,
            _id: rim_core::keybind::model::KeybindId,
            _update: UpdateKeybind,
        ) -> RepoResult<Keybind> {
            Ok(self.created.lock().unwrap().clone().unwrap())
        }

        async fn delete_keybind(&self, _id: rim_core::keybind::model::KeybindId) -> RepoResult<()> {
            Ok(())
        }
    }

    struct FakeCommandService {
        commands: Vec<CommandDefinitionDto>,
    }

    #[async_trait]
    impl CommandService for FakeCommandService {
        async fn list_commands(
            &self,
            _query: ListCommandsQuery,
        ) -> ServiceResult<Vec<CommandDefinitionDto>> {
            Ok(self.commands.clone())
        }

        async fn execute_command(
            &self,
            _cmd: ExecuteCommandCmd,
        ) -> ServiceResult<CommandResultDto> {
            Err(ServiceError::Validation("unsupported".to_string()))
        }

        async fn create_user_command(
            &self,
            _cmd: CreateUserCommandCmd,
        ) -> ServiceResult<UserCommandDto> {
            Err(ServiceError::Validation("unsupported".to_string()))
        }

        async fn update_user_command(
            &self,
            _cmd: UpdateUserCommandCmd,
        ) -> ServiceResult<UserCommandDto> {
            Err(ServiceError::Validation("unsupported".to_string()))
        }

        async fn delete_user_command(&self, _cmd: DeleteUserCommandCmd) -> ServiceResult<()> {
            Err(ServiceError::Validation("unsupported".to_string()))
        }
    }

    struct FakeServerMemberRepository;

    #[async_trait]
    impl ServerMemberRepository for FakeServerMemberRepository {
        async fn get_by_id(&self, _id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _server_id: rim_core::server::model::ServerId,
            _user_id: rim_core::user::model::UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(
            &self,
            _server_id: rim_core::server::model::ServerId,
            _user_id: rim_core::user::model::UserId,
        ) -> RepoResult<()> {
            Ok(())
        }

        async fn get_members_for_server(
            &self,
            _server_id: rim_core::server::model::ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            Ok(vec![])
        }

        async fn get_member(
            &self,
            server_id: rim_core::server::model::ServerId,
            user_id: rim_core::user::model::UserId,
        ) -> RepoResult<Option<ServerMember>> {
            Ok(Some(ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: ServerMemberRole::Owner,
                joined_at: chrono::Utc::now(),
            }))
        }

        async fn change_member_role(
            &self,
            _server_id: rim_core::server::model::ServerId,
            _user_id: rim_core::user::model::UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Ok(())
        }

        async fn is_member(
            &self,
            _server_id: rim_core::server::model::ServerId,
            _user_id: rim_core::user::model::UserId,
        ) -> RepoResult<bool> {
            Ok(true)
        }
    }

    fn actor() -> rim_services::Actor {
        rim_services::Actor::new(rim_core::user::model::UserId::new())
    }

    fn service(
        keybind_repo: FakeKeybindRepository,
        command_service: FakeCommandService,
    ) -> DefaultKeybindService {
        DefaultKeybindService::new(
            KeybindRepos {
                keybind_repo: std::sync::Arc::new(keybind_repo),
                server_member_repo: std::sync::Arc::new(FakeServerMemberRepository),
            },
            KeybindServices {
                command_service: std::sync::Arc::new(command_service),
            },
        )
    }

    fn bindable_command(id: &str) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: id.to_string(),
            title: "Command".to_string(),
            description: "desc".to_string(),
            category: "Workspace".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
            required_context: vec![],
            required_capabilities: vec![],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: true,
            dangerous: false,
            user_command_id: None,
        }
    }

    #[tokio::test]
    async fn create_keybind_requires_keybind_capable_command() {
        let keybind_repo = FakeKeybindRepository {
            existing: std::sync::Mutex::new(None),
            created: std::sync::Mutex::new(None),
        };
        let command_service = FakeCommandService { commands: vec![] };

        let svc = service(keybind_repo, command_service);
        let err = svc
            .create_keybind(CreateKeybindCmd {
                actor: actor(),
                scope: KeybindScopeDto::User,
                workspace_id: None,
                keystroke: "Ctrl+K".to_string(),
                command_id: "workspace.open_command_palette".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_keybind_creates_user_keybind() {
        let actor = actor();
        let keybind_repo = FakeKeybindRepository {
            existing: std::sync::Mutex::new(None),
            created: std::sync::Mutex::new(Some(Keybind {
                id: rim_core::keybind::model::KeybindId::new(),
                scope: KeybindScope::User,
                subject: KeybindSubject::User(actor.user_id),
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.cycle_recent_activity".to_string(),
                context: KeybindContext::Global,
                preset_args: None,
                is_enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })),
        };
        let command_service = FakeCommandService {
            commands: vec![bindable_command("workspace.cycle_recent_activity")],
        };

        let svc = service(keybind_repo, command_service);
        let dto = svc
            .create_keybind(CreateKeybindCmd {
                actor,
                scope: KeybindScopeDto::User,
                workspace_id: None,
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.cycle_recent_activity".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            })
            .await
            .unwrap();

        assert_eq!(dto.command_id, "workspace.cycle_recent_activity");
        assert_eq!(dto.scope, KeybindScopeDto::User);
    }
}
