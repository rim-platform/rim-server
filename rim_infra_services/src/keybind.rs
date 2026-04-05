use async_trait::async_trait;

use rim_core::keybind::model::{Keybind, KeybindSubject, NewKeybind, UpdateKeybind};
use rim_services::command::request::GetCommandQuery;
use rim_services::command::service::CommandService;
use rim_services::keybind::dto::{
    DefaultKeybindDto, KeybindDto, KeybindLayersDto, KeybindScopeDto, KeybindSourceDto,
};
use rim_services::keybind::request::{
    CreateKeybindCmd, DeleteKeybindCmd, InspectKeybindLayersQuery, ListDefaultKeybindsQuery,
    ListKeybindsQuery, ResolveKeybindQuery, ResolveKeybindsQuery, UpdateKeybindCmd,
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
    const MAX_USER_KEYBINDS: usize = 200;
    const MAX_WORKSPACE_KEYBINDS: usize = 100;
    const MAX_PRESET_ARGS_BYTES: usize = 4096;
    const MAX_KEYSTROKE_LEN: usize = 50;

    const VALID_MODIFIERS: &'static [&'static str] = &["Ctrl", "Shift", "Alt", "Meta"];

    pub fn new(repos: KeybindRepos, services: KeybindServices) -> Self {
        Self { repos, services }
    }

    fn validate_keystroke(keystroke: &str) -> ServiceResult<()> {
        let keystroke = keystroke.trim();
        if keystroke.is_empty() {
            return Err(ServiceError::Validation(
                "keystroke must be non-empty".to_string(),
            ));
        }
        if keystroke.len() > Self::MAX_KEYSTROKE_LEN {
            return Err(ServiceError::Validation(format!(
                "keystroke exceeds maximum length of {} characters",
                Self::MAX_KEYSTROKE_LEN
            )));
        }

        let parts: Vec<&str> = keystroke.split('+').collect();
        if parts.is_empty() {
            return Err(ServiceError::Validation(
                "keystroke must contain at least one key".to_string(),
            ));
        }

        let (modifiers, key_parts) = parts.split_at(parts.len() - 1);

        for modifier in modifiers {
            if !Self::VALID_MODIFIERS.contains(modifier) {
                return Err(ServiceError::Validation(format!(
                    "invalid modifier `{modifier}` in keystroke; allowed: Ctrl, Shift, Alt, Meta"
                )));
            }
        }

        let key = key_parts[0];
        if key.is_empty() {
            return Err(ServiceError::Validation(
                "keystroke must end with a key name".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_preset_args_size(preset_args: &Option<serde_json::Value>) -> ServiceResult<()> {
        if let Some(args) = preset_args {
            let size = serde_json::to_vec(args).map(|v| v.len()).unwrap_or(0);
            if size > Self::MAX_PRESET_ARGS_BYTES {
                return Err(ServiceError::Validation(format!(
                    "preset_args exceeds maximum size of {} bytes",
                    Self::MAX_PRESET_ARGS_BYTES
                )));
            }
        }
        Ok(())
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

    fn default_keybinds() -> Vec<DefaultKeybindDto> {
        vec![
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+K".to_string(),
                command_id: "workspace.open_command_palette".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Alt+S".to_string(),
                command_id: "workspace.open_settings".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+F".to_string(),
                command_id: "workspace.open_search".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+F".to_string(),
                command_id: "workspace.search_in_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+X".to_string(),
                command_id: "workspace.open_marketplace".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.cycle_recent_activity".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+A".to_string(),
                command_id: "workspace.open_agent".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+B".to_string(),
                command_id: "workspace.toggle_sidebar".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+T".to_string(),
                command_id: "workspace.reopen_last_thread".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+E".to_string(),
                command_id: "workspace.focus_composer".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+K".to_string(),
                command_id: "workspace.navigate_to_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Alt+ArrowUp".to_string(),
                command_id: "workspace.previous_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Alt+ArrowDown".to_string(),
                command_id: "workspace.next_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Alt+Shift+ArrowDown".to_string(),
                command_id: "workspace.next_unread_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Escape".to_string(),
                command_id: "workspace.close_panel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+M".to_string(),
                command_id: "chat.mark_all_as_read".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+G".to_string(),
                command_id: "chat.create_group".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
            DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+Shift+C".to_string(),
                command_id: "chat.create_channel".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            },
        ]
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
            source: match keybind.subject {
                KeybindSubject::User(_) => KeybindSourceDto::User,
                KeybindSubject::Workspace(_) => KeybindSourceDto::Workspace,
            },
            workspace_id,
            keystroke: keybind.keystroke,
            command_id: keybind.command_id,
            context: keybind.context.into(),
            preset_args: keybind.preset_args,
            is_enabled: keybind.is_enabled,
        }
    }

    fn sort_keybinds(keybinds: &mut [Keybind]) {
        keybinds.sort_by(|left, right| {
            left.context
                .to_string()
                .cmp(&right.context.to_string())
                .then(left.keystroke.cmp(&right.keystroke))
                .then(left.command_id.cmp(&right.command_id))
        });
    }

    async fn build_resolved_keybinds_map(
        &self,
        query: ResolveKeybindsQuery,
    ) -> ServiceResult<std::collections::BTreeMap<(String, String), KeybindDto>> {
        let mut effective = std::collections::BTreeMap::new();

        for keybind in Self::default_keybinds() {
            effective.insert(
                (format!("{:?}", keybind.context), keybind.keystroke.clone()),
                KeybindDto {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: KeybindScopeDto::User,
                    source: keybind.source,
                    workspace_id: None,
                    keystroke: keybind.keystroke,
                    command_id: keybind.command_id,
                    context: keybind.context,
                    preset_args: keybind.preset_args,
                    is_enabled: true,
                },
            );
        }

        let mut user_keybinds = self
            .repos
            .keybind_repo
            .list_for_subject(KeybindSubject::User(query.actor.user_id))
            .await
            .map_err(rim_services::map_repo_error)?;
        Self::sort_keybinds(&mut user_keybinds);
        for keybind in user_keybinds
            .into_iter()
            .filter(|keybind| keybind.is_enabled)
        {
            effective.insert(
                (format!("{:?}", keybind.context), keybind.keystroke.clone()),
                Self::to_dto(keybind),
            );
        }

        if let Some(workspace_id) = query.workspace_id {
            ensure_server_member(
                self.repos.server_member_repo.as_ref(),
                workspace_id,
                query.actor.user_id,
            )
            .await?;

            let mut workspace_keybinds = self
                .repos
                .keybind_repo
                .list_for_subject(KeybindSubject::Workspace(workspace_id))
                .await
                .map_err(rim_services::map_repo_error)?;
            Self::sort_keybinds(&mut workspace_keybinds);
            for keybind in workspace_keybinds
                .into_iter()
                .filter(|keybind| keybind.is_enabled)
            {
                effective.insert(
                    (format!("{:?}", keybind.context), keybind.keystroke.clone()),
                    Self::to_dto(keybind),
                );
            }
        }

        Ok(effective)
    }

    async fn inspect_layers(
        &self,
        actor: rim_services::Actor,
        workspace_id: Option<rim_core::server::model::WorkspaceId>,
        keystroke: &str,
        context: rim_services::keybind::dto::KeybindContextDto,
    ) -> ServiceResult<KeybindLayersDto> {
        let default_keybind = Self::default_keybinds()
            .into_iter()
            .find(|keybind| keybind.keystroke == keystroke && keybind.context == context);

        let user_keybind = self
            .repos
            .keybind_repo
            .get_by_binding(
                rim_core::keybind::model::KeybindScope::User,
                KeybindSubject::User(actor.user_id),
                context.into(),
                keystroke,
            )
            .await
            .map_err(rim_services::map_repo_error)?
            .filter(|keybind| keybind.is_enabled)
            .map(Self::to_dto);

        let workspace_keybind = if let Some(workspace_id) = workspace_id {
            ensure_server_member(
                self.repos.server_member_repo.as_ref(),
                workspace_id,
                actor.user_id,
            )
            .await?;

            self.repos
                .keybind_repo
                .get_by_binding(
                    rim_core::keybind::model::KeybindScope::Workspace,
                    KeybindSubject::Workspace(workspace_id),
                    context.into(),
                    keystroke,
                )
                .await
                .map_err(rim_services::map_repo_error)?
                .filter(|keybind| keybind.is_enabled)
                .map(Self::to_dto)
        } else {
            None
        };

        let resolved_persisted_keybind = workspace_keybind
            .clone()
            .or_else(|| user_keybind.clone())
            .or_else(|| {
                default_keybind.clone().map(|keybind| KeybindDto {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: KeybindScopeDto::User,
                    source: keybind.source,
                    workspace_id: None,
                    keystroke: keybind.keystroke,
                    command_id: keybind.command_id,
                    context: keybind.context,
                    preset_args: keybind.preset_args,
                    is_enabled: true,
                })
            });

        Ok(KeybindLayersDto {
            default_keybind,
            user_keybind,
            workspace_keybind,
            resolved_persisted_keybind,
        })
    }

    async fn validate_command_bindable(
        &self,
        actor: rim_services::Actor,
        command_id: &str,
    ) -> ServiceResult<()> {
        let command = self
            .services
            .command_service
            .get_command(GetCommandQuery {
                actor,
                command_id: command_id.to_string(),
            })
            .await?;

        if command.is_some_and(|command| command.keybind_capable) {
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
    async fn list_default_keybinds(
        &self,
        _query: ListDefaultKeybindsQuery,
    ) -> ServiceResult<Vec<DefaultKeybindDto>> {
        Ok(Self::default_keybinds())
    }

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
        Self::sort_keybinds(&mut keybinds);
        Ok(keybinds.into_iter().map(Self::to_dto).collect())
    }

    async fn resolve_keybinds(
        &self,
        query: ResolveKeybindsQuery,
    ) -> ServiceResult<Vec<KeybindDto>> {
        Ok(self
            .build_resolved_keybinds_map(query)
            .await?
            .into_values()
            .collect())
    }

    async fn resolve_keybind(
        &self,
        query: ResolveKeybindQuery,
    ) -> ServiceResult<Option<KeybindDto>> {
        Ok(self
            .inspect_layers(
                query.actor,
                query.workspace_id,
                &query.keystroke,
                query.context,
            )
            .await?
            .resolved_persisted_keybind)
    }

    async fn inspect_keybind_layers(
        &self,
        query: InspectKeybindLayersQuery,
    ) -> ServiceResult<KeybindLayersDto> {
        self.inspect_layers(
            query.actor,
            query.workspace_id,
            &query.keystroke,
            query.context,
        )
        .await
    }

    async fn create_keybind(&self, cmd: CreateKeybindCmd) -> ServiceResult<KeybindDto> {
        let subject = Self::subject_for_request(cmd.actor.user_id, cmd.scope, cmd.workspace_id)?;
        self.ensure_scope_access(cmd.actor, subject).await?;
        self.validate_command_bindable(cmd.actor, &cmd.command_id)
            .await?;
        Self::validate_keystroke(&cmd.keystroke)?;
        Self::validate_preset_args_size(&cmd.preset_args)?;

        let existing_count = self
            .repos
            .keybind_repo
            .list_for_subject(subject)
            .await
            .map_err(rim_services::map_repo_error)?
            .len();
        let limit = match subject {
            KeybindSubject::User(_) => Self::MAX_USER_KEYBINDS,
            KeybindSubject::Workspace(_) => Self::MAX_WORKSPACE_KEYBINDS,
        };
        if existing_count >= limit {
            return Err(ServiceError::Validation(format!(
                "maximum number of keybinds ({limit}) reached for this scope"
            )));
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
        Self::validate_keystroke(&cmd.keystroke)?;
        Self::validate_preset_args_size(&cmd.preset_args)?;

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
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_services::command::dto::{
        CommandDefinitionDto, CommandExecutionTargetDto, CommandSourceKindDto,
    };
    use rim_services::command::service::MockCommandService;
    use rim_services::keybind::dto::{KeybindContextDto, KeybindScopeDto};
    use rim_services::keybind::request::InspectKeybindLayersQuery;

    struct FakeKeybindRepository {
        entries: std::sync::Mutex<Vec<Keybind>>,
        created: std::sync::Mutex<Option<Keybind>>,
    }

    #[async_trait]
    impl KeybindRepository for FakeKeybindRepository {
        async fn get_by_id(
            &self,
            id: rim_core::keybind::model::KeybindId,
        ) -> RepoResult<Option<Keybind>> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|k| k.id == id)
                .cloned())
        }

        async fn list_for_subject(&self, _subject: KeybindSubject) -> RepoResult<Vec<Keybind>> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|keybind| keybind.subject == _subject)
                .cloned()
                .collect())
        }

        async fn get_by_binding(
            &self,
            _scope: KeybindScope,
            _subject: KeybindSubject,
            _context: KeybindContext,
            _keystroke: &str,
        ) -> RepoResult<Option<Keybind>> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|keybind| {
                    keybind.scope == _scope
                        && keybind.subject == _subject
                        && keybind.context == _context
                        && keybind.keystroke == _keystroke
                })
                .cloned())
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

    mockall::mock! {
        ServerMemberRepository {}
        #[async_trait]
        impl ServerMemberRepository for ServerMemberRepository {
            async fn get_by_id(&self, id: ServerMemberId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(
                &self,
                server_id: rim_core::server::model::ServerId,
                user_id: rim_core::user::model::UserId,
                role: ServerMemberRole,
            ) -> RepoResult<ServerMember>;
            async fn remove_member(
                &self,
                server_id: rim_core::server::model::ServerId,
                user_id: rim_core::user::model::UserId,
            ) -> RepoResult<()>;
            async fn get_members_for_server(
                &self,
                server_id: rim_core::server::model::ServerId,
            ) -> RepoResult<Vec<ServerMember>>;
            async fn get_member(
                &self,
                server_id: rim_core::server::model::ServerId,
                user_id: rim_core::user::model::UserId,
            ) -> RepoResult<Option<ServerMember>>;
            async fn change_member_role(
                &self,
                server_id: rim_core::server::model::ServerId,
                user_id: rim_core::user::model::UserId,
                role: ServerMemberRole,
            ) -> RepoResult<()>;
            async fn is_member(
                &self,
                server_id: rim_core::server::model::ServerId,
                user_id: rim_core::user::model::UserId,
            ) -> RepoResult<bool>;
        }
    }

    fn actor() -> rim_services::Actor {
        rim_services::Actor::new(rim_core::user::model::UserId::new())
    }

    fn service(
        keybind_repo: FakeKeybindRepository,
        command_service: MockCommandService,
    ) -> DefaultKeybindService {
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(|server_id, user_id| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role: ServerMemberRole::Owner,
                    joined_at: chrono::Utc::now(),
                }))
            });
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        DefaultKeybindService::new(
            KeybindRepos {
                keybind_repo: std::sync::Arc::new(keybind_repo),
                server_member_repo: std::sync::Arc::new(server_member_repo),
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
            entries: std::sync::Mutex::new(vec![]),
            created: std::sync::Mutex::new(None),
        };
        let mut command_service = MockCommandService::new();
        command_service.expect_get_command().return_once(|query| {
            assert_eq!(query.command_id, "workspace.open_command_palette");
            Box::pin(async { Ok(None) })
        });

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
            entries: std::sync::Mutex::new(vec![]),
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
        let mut command_service = MockCommandService::new();
        command_service.expect_get_command().return_once(|query| {
            assert_eq!(query.command_id, "workspace.cycle_recent_activity");
            Box::pin(async { Ok(Some(bindable_command("workspace.cycle_recent_activity"))) })
        });

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

    #[tokio::test]
    async fn resolve_keybinds_prefers_workspace_over_user() {
        let actor = actor();
        let workspace_id = rim_core::server::model::WorkspaceId::new();
        let keybind_repo = FakeKeybindRepository {
            entries: std::sync::Mutex::new(vec![
                Keybind {
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
                },
                Keybind {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: KeybindScope::Workspace,
                    subject: KeybindSubject::Workspace(workspace_id),
                    keystroke: "Ctrl+R".to_string(),
                    command_id: "workspace.open_command_palette".to_string(),
                    context: KeybindContext::Global,
                    preset_args: None,
                    is_enabled: true,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                },
            ]),
            created: std::sync::Mutex::new(None),
        };
        let command_service = MockCommandService::new();
        let svc = service(keybind_repo, command_service);

        let resolved = svc
            .resolve_keybinds(ResolveKeybindsQuery {
                actor,
                workspace_id: Some(workspace_id),
            })
            .await
            .unwrap();

        assert_eq!(resolved.len(), 18);
        let ctrl_r = resolved.iter().find(|k| k.keystroke == "Ctrl+R").unwrap();
        assert_eq!(ctrl_r.scope, KeybindScopeDto::Workspace);
        assert_eq!(ctrl_r.source, KeybindSourceDto::Workspace);
        let ctrl_k = resolved.iter().find(|k| k.keystroke == "Ctrl+K").unwrap();
        assert_eq!(ctrl_k.source, KeybindSourceDto::Default);
    }

    #[tokio::test]
    async fn resolve_single_keybind_returns_highest_precedence_binding() {
        let actor = actor();
        let workspace_id = rim_core::server::model::WorkspaceId::new();
        let keybind_repo = FakeKeybindRepository {
            entries: std::sync::Mutex::new(vec![Keybind {
                id: rim_core::keybind::model::KeybindId::new(),
                scope: KeybindScope::Workspace,
                subject: KeybindSubject::Workspace(workspace_id),
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.open_command_palette".to_string(),
                context: KeybindContext::Global,
                preset_args: None,
                is_enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }]),
            created: std::sync::Mutex::new(None),
        };
        let svc = service(keybind_repo, MockCommandService::new());

        let resolved = svc
            .resolve_keybind(ResolveKeybindQuery {
                actor,
                workspace_id: Some(workspace_id),
                keystroke: "Ctrl+R".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(resolved.command_id, "workspace.open_command_palette");
        assert_eq!(resolved.source, KeybindSourceDto::Workspace);
    }

    #[tokio::test]
    async fn inspect_keybind_layers_returns_layered_state() {
        let actor = actor();
        let workspace_id = rim_core::server::model::WorkspaceId::new();
        let keybind_repo = FakeKeybindRepository {
            entries: std::sync::Mutex::new(vec![
                Keybind {
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
                },
                Keybind {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: KeybindScope::Workspace,
                    subject: KeybindSubject::Workspace(workspace_id),
                    keystroke: "Ctrl+R".to_string(),
                    command_id: "workspace.open_command_palette".to_string(),
                    context: KeybindContext::Global,
                    preset_args: None,
                    is_enabled: true,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                },
            ]),
            created: std::sync::Mutex::new(None),
        };
        let svc = service(keybind_repo, MockCommandService::new());

        let layers = svc
            .inspect_keybind_layers(InspectKeybindLayersQuery {
                actor,
                workspace_id: Some(workspace_id),
                keystroke: "Ctrl+R".to_string(),
                context: KeybindContextDto::Global,
            })
            .await
            .unwrap();

        assert!(layers.default_keybind.is_some());
        assert!(layers.user_keybind.is_some());
        assert!(layers.workspace_keybind.is_some());
        assert_eq!(
            layers.resolved_persisted_keybind.as_ref().map(|k| k.source),
            Some(KeybindSourceDto::Workspace)
        );
    }

    #[tokio::test]
    async fn list_default_keybinds_returns_defaults() {
        let svc = service(
            FakeKeybindRepository {
                entries: std::sync::Mutex::new(vec![]),
                created: std::sync::Mutex::new(None),
            },
            MockCommandService::new(),
        );

        let defaults = svc
            .list_default_keybinds(ListDefaultKeybindsQuery { actor: actor() })
            .await
            .unwrap();

        assert!(
            defaults
                .iter()
                .any(|k| k.command_id == "workspace.open_command_palette")
        );
        assert!(
            defaults
                .iter()
                .all(|k| k.source == KeybindSourceDto::Default)
        );
    }
}
