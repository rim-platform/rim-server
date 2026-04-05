use std::sync::Arc;

use async_trait::async_trait;

use rim_core::macro_def::model::{NewMacroDef, UpdateMacroDef};
use rim_core::macro_def::repository::MacroRepository;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::macro_def::dto::{MacroDto, MacroStepDto};
use rim_services::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, ListServerMacrosQuery, ListUserMacrosQuery,
    UpdateMacroCmd,
};
use rim_services::macro_def::service::MacroService;
use rim_services::{ServiceError, ServiceResult, map_repo_error};

const MAX_STEPS: usize = 20;
const MAX_MACROS_PER_USER: u64 = 50;

#[derive(Clone)]
pub struct MacroRepos {
    pub macro_repo: Arc<dyn MacroRepository>,
}

#[derive(Clone)]
pub struct MacroServices {
    pub command_service: Arc<dyn CommandService>,
}

#[derive(Clone)]
pub struct DefaultMacroService {
    repos: MacroRepos,
    services: MacroServices,
}

impl DefaultMacroService {
    pub fn new(repos: MacroRepos, services: MacroServices) -> Self {
        Self { repos, services }
    }

    fn parse_steps(steps_json: &serde_json::Value) -> ServiceResult<Vec<MacroStepDto>> {
        #[derive(serde::Deserialize)]
        struct PersistedStep {
            #[serde(rename = "commandId")]
            command_id: String,
            args: Option<serde_json::Value>,
        }

        let persisted: Vec<PersistedStep> =
            serde_json::from_value(steps_json.clone()).map_err(|e| {
                ServiceError::Internal(Some(anyhow::anyhow!("failed to parse macro steps: {e}")))
            })?;

        Ok(persisted
            .into_iter()
            .map(|s| MacroStepDto {
                command_id: s.command_id,
                args: s.args,
            })
            .collect())
    }

    fn to_dto(m: rim_core::macro_def::model::MacroDef) -> ServiceResult<MacroDto> {
        let steps = Self::parse_steps(&m.steps_json)?;
        Ok(MacroDto {
            id: m.id,
            server_id: m.server_id,
            user_id: m.user_id,
            name: m.name,
            description: m.description,
            steps,
            created_at: m.created_at,
        })
    }

    fn serialize_steps(steps: &[MacroStepDto]) -> ServiceResult<serde_json::Value> {
        #[derive(serde::Serialize)]
        struct PersistedStep {
            #[serde(rename = "commandId")]
            command_id: String,
            args: Option<serde_json::Value>,
        }

        let persisted: Vec<PersistedStep> = steps
            .iter()
            .map(|s| PersistedStep {
                command_id: s.command_id.clone(),
                args: s.args.clone(),
            })
            .collect();

        serde_json::to_value(persisted).map_err(ServiceError::internal)
    }

    fn validate_steps(steps: &[MacroStepDto]) -> ServiceResult<()> {
        if steps.is_empty() {
            return Err(ServiceError::Validation(
                "macro must have at least one step".to_string(),
            ));
        }
        if steps.len() > MAX_STEPS {
            return Err(ServiceError::Validation(format!(
                "macro may have at most {MAX_STEPS} steps"
            )));
        }
        for step in steps {
            if step.command_id.starts_with("macro.") {
                return Err(ServiceError::Validation(
                    "macro steps must not reference other macros".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl MacroService for DefaultMacroService {
    async fn create_macro(&self, cmd: CreateMacroCmd) -> ServiceResult<MacroDto> {
        let name = cmd.name.trim().to_string();
        if name.is_empty() {
            return Err(ServiceError::Validation(
                "macro name must be non-empty".to_string(),
            ));
        }
        if name.len() > 128 {
            return Err(ServiceError::Validation(
                "macro name must be at most 128 characters".to_string(),
            ));
        }

        Self::validate_steps(&cmd.steps)?;

        let count = self
            .repos
            .macro_repo
            .count_for_user(cmd.server_id, cmd.actor.user_id)
            .await
            .map_err(map_repo_error)?;
        if count >= MAX_MACROS_PER_USER {
            return Err(ServiceError::Validation(format!(
                "user may have at most {MAX_MACROS_PER_USER} macros per server"
            )));
        }

        let steps_json = Self::serialize_steps(&cmd.steps)?;

        let created = self
            .repos
            .macro_repo
            .create(NewMacroDef {
                server_id: cmd.server_id,
                user_id: cmd.actor.user_id,
                name,
                description: cmd.description,
                steps_json,
            })
            .await
            .map_err(map_repo_error)?;

        Self::to_dto(created)
    }

    async fn update_macro(&self, cmd: UpdateMacroCmd) -> ServiceResult<MacroDto> {
        let existing = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if existing.user_id != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        let name = cmd.name.trim().to_string();
        if name.is_empty() {
            return Err(ServiceError::Validation(
                "macro name must be non-empty".to_string(),
            ));
        }
        if name.len() > 128 {
            return Err(ServiceError::Validation(
                "macro name must be at most 128 characters".to_string(),
            ));
        }

        Self::validate_steps(&cmd.steps)?;

        let steps_json = Self::serialize_steps(&cmd.steps)?;

        let updated = self
            .repos
            .macro_repo
            .update(
                cmd.macro_id,
                UpdateMacroDef {
                    name,
                    description: cmd.description,
                    steps_json,
                },
            )
            .await
            .map_err(map_repo_error)?;

        Self::to_dto(updated)
    }

    async fn delete_macro(&self, cmd: DeleteMacroCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if existing.user_id != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos
            .macro_repo
            .delete(cmd.macro_id)
            .await
            .map_err(map_repo_error)
    }

    async fn list_user_macros(&self, query: ListUserMacrosQuery) -> ServiceResult<Vec<MacroDto>> {
        let macros = self
            .repos
            .macro_repo
            .list_for_user(query.server_id, query.actor.user_id)
            .await
            .map_err(map_repo_error)?;

        macros.into_iter().map(Self::to_dto).collect()
    }

    async fn list_server_macros(
        &self,
        query: ListServerMacrosQuery,
    ) -> ServiceResult<Vec<MacroDto>> {
        let macros = self
            .repos
            .macro_repo
            .list_for_server(query.server_id)
            .await
            .map_err(map_repo_error)?;

        macros.into_iter().map(Self::to_dto).collect()
    }

    async fn execute_macro(&self, cmd: ExecuteMacroCmd) -> ServiceResult<CommandResultDto> {
        let macro_def = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let steps = Self::parse_steps(&macro_def.steps_json)?;

        let mut payload = serde_json::Map::new();
        for step in steps {
            let result = self
                .services
                .command_service
                .execute_command(ExecuteCommandCmd {
                    actor: cmd.actor,
                    command_id: step.command_id.clone(),
                    args: step.args.clone(),
                    context: cmd.command_context.clone(),
                    confirmed_dangerous: cmd.confirmed_dangerous,
                })
                .await?;

            if let Some(step_payload) = result.payload {
                payload.insert(step.command_id, step_payload);
            }
        }

        Ok(CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: format!("macro `{}` executed", macro_def.name),
            payload: Some(serde_json::Value::Object(payload)),
            follow_up_commands: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::macro_def::model::{MacroDef, MacroId};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::command::service::MockCommandService;
    use rim_services::Actor;

    use async_trait::async_trait;
    use rim_core::macro_def::repository::MacroRepository;
    use rim_core::repository::error::RepoResult;

    mockall::mock! {
        MacroRepository {}
        #[async_trait]
        impl MacroRepository for MacroRepository {
            async fn get_by_id(&self, id: MacroId) -> RepoResult<Option<MacroDef>>;
            async fn list_for_user(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<Vec<MacroDef>>;
            async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<MacroDef>>;
            async fn count_for_user(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<u64>;
            async fn create(&self, new_macro: rim_core::macro_def::model::NewMacroDef) -> RepoResult<MacroDef>;
            async fn update(&self, id: MacroId, update: rim_core::macro_def::model::UpdateMacroDef) -> RepoResult<MacroDef>;
            async fn delete(&self, id: MacroId) -> RepoResult<()>;
        }
    }

    fn service(
        repo: MockMacroRepository,
        command_service: MockCommandService,
    ) -> DefaultMacroService {
        DefaultMacroService::new(
            MacroRepos {
                macro_repo: Arc::new(repo),
            },
            MacroServices {
                command_service: Arc::new(command_service),
            },
        )
    }

    #[tokio::test]
    async fn create_macro_validates_empty_name() {
        let repo = MockMacroRepository::new();
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let err = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "  ".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_macro_rejects_empty_steps() {
        let repo = MockMacroRepository::new();
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let err = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "Test".to_string(),
                description: None,
                steps: vec![],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_macro_rejects_macro_recursion() {
        let repo = MockMacroRepository::new();
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let err = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "Bad".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "macro.other".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_macro_rejects_too_many_steps() {
        let repo = MockMacroRepository::new();
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let steps: Vec<MacroStepDto> = (0..21)
            .map(|i| MacroStepDto {
                command_id: format!("chat.post_message_{i}"),
                args: None,
            })
            .collect();

        let err = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "Too many".to_string(),
                description: None,
                steps,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_macro_enforces_per_user_limit() {
        let mut repo = MockMacroRepository::new();
        repo.expect_count_for_user().returning(|_, _| Ok(50));
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let err = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "One more".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_macro_succeeds() {
        let mut repo = MockMacroRepository::new();
        repo.expect_count_for_user().returning(|_, _| Ok(0));
        repo.expect_create().returning(|new_macro| {
            Ok(MacroDef {
                id: MacroId::new(),
                server_id: new_macro.server_id,
                user_id: new_macro.user_id,
                name: new_macro.name,
                description: new_macro.description,
                steps_json: new_macro.steps_json,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        });
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let actor = Actor::new(UserId::new());
        let result = svc
            .create_macro(CreateMacroCmd {
                actor,
                server_id: ServerId::new(),
                name: "My Macro".to_string(),
                description: Some("Does stuff".to_string()),
                steps: vec![MacroStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap();

        assert_eq!(result.name, "My Macro");
        assert_eq!(result.steps.len(), 1);
    }

    #[tokio::test]
    async fn delete_macro_checks_ownership() {
        let owner = UserId::new();
        let other = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "test".to_string(),
                description: None,
                steps_json: serde_json::json!([]),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });
        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let err = svc
            .delete_macro(DeleteMacroCmd {
                actor: Actor::new(other),
                macro_id,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn execute_macro_runs_all_steps() {
        let owner = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "test".to_string(),
                description: None,
                steps_json: serde_json::json!([
                    { "commandId": "chat.post_message", "args": { "text": "hello" } },
                    { "commandId": "chat.post_message", "args": { "text": "world" } }
                ]),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc.expect_execute_command().times(2).returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                })
            })
        });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmed_dangerous: false,
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }
}
