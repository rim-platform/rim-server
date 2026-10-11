use std::sync::Arc;

use async_trait::async_trait;

use rim_core::macro_def::model::{NewMacroDef, UpdateMacroDef};
use rim_core::macro_def::repository::MacroRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::confirmation::{
    ConfirmationProof, ConfirmationStore, issue_in_band, verify_in_band,
};
use rim_services::command::dto::{
    CommandDefinitionDto, CommandExecutionTargetDto, CommandResultDto, CommandResultStatusDto,
    CommandSourceKindDto,
};
use rim_services::command::request::{ExecuteCommandCmd, GetCommandQuery};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::service::CommandService;
use rim_services::command::verb::TypedVerb;
use rim_services::macro_def::dto::{MacroDto, MacroStepDto};
use rim_services::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, GetMacroByNameQuery, ListUserMacrosQuery,
    SynthesizeMacroCommandsQuery, UpdateMacroCmd,
};
use rim_services::macro_def::service::MacroService;
use rim_services::{ServiceError, ServiceResult};

const MAX_STEPS: usize = 20;
const MAX_MACROS_PER_USER: u64 = 50;

#[derive(Clone)]
pub struct MacroRepos {
    pub macro_repo: Arc<dyn MacroRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct MacroServices {
    pub command_service: Arc<dyn CommandService>,
    pub audit_log_service: Option<Arc<dyn AuditLogService>>,
    /// Shared confirmation token store for verifying macro-level proofs.
    /// Same store instance as `DefaultCommandService` in production.
    /// `None` in tests that don't exercise the confirmation gate.
    pub confirmation_store: Option<Arc<dyn ConfirmationStore>>,
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
        let sensitivity = CommandSensitivityDto::from(m.sensitivity.as_str());
        Ok(MacroDto {
            id: m.id,
            server_id: m.server_id,
            user_id: m.user_id,
            name: m.name,
            description: m.description,
            steps,
            ai_callable: m.ai_callable,
            nl_description: m.nl_description,
            intent_samples: m.intent_samples,
            sensitivity,
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

        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let count = self
            .repos
            .macro_repo
            .count_for_user(cmd.server_id, cmd.actor.user_id)
            .await?;
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
                ai_callable: cmd.ai_callable,
                nl_description: cmd.nl_description,
                intent_samples: cmd.intent_samples,
                sensitivity: cmd.sensitivity.as_str().to_string(),
            })
            .await?;

        Self::to_dto(created)
    }

    async fn update_macro(&self, cmd: UpdateMacroCmd) -> ServiceResult<MacroDto> {
        let existing = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await?
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
                    ai_callable: cmd.ai_callable,
                    nl_description: cmd.nl_description,
                    intent_samples: cmd.intent_samples,
                    sensitivity: cmd.sensitivity.as_str().to_string(),
                },
            )
            .await?;

        Self::to_dto(updated)
    }

    async fn delete_macro(&self, cmd: DeleteMacroCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if existing.user_id != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos
            .macro_repo
            .delete(cmd.macro_id)
            .await
            .map_err(ServiceError::from)
    }

    async fn get_macro_by_name(
        &self,
        query: GetMacroByNameQuery,
    ) -> ServiceResult<Option<MacroDto>> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        if query.name.len() > 128 {
            return Err(ServiceError::Validation(
                "macro name exceeds maximum length of 128 characters".into(),
            ));
        }

        let macro_def = self
            .repos
            .macro_repo
            .find_by_name(query.server_id, query.actor.user_id, &query.name)
            .await?;

        macro_def.map(Self::to_dto).transpose()
    }

    async fn list_user_macros(&self, query: ListUserMacrosQuery) -> ServiceResult<Vec<MacroDto>> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let macros = self
            .repos
            .macro_repo
            .list_for_user(query.server_id, query.actor.user_id)
            .await?;

        macros.into_iter().map(Self::to_dto).collect()
    }

    async fn execute_macro(&self, cmd: ExecuteMacroCmd) -> ServiceResult<CommandResultDto> {
        let macro_def = self
            .repos
            .macro_repo
            .get_by_id(cmd.macro_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if macro_def.user_id != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        if cmd.initiator.is_agentic() && !macro_def.ai_callable {
            return Err(ServiceError::PermissionDenied);
        }

        let steps = Self::parse_steps(&macro_def.steps_json)?;

        // ── Resolve each step's declared sensitivity via the command
        // service, then compute effective sensitivity as max(macro_tier, step_tiers).
        // Prevents a Safe-declared macro from silently wrapping a Restricted step.
        let macro_sensitivity = CommandSensitivityDto::from(macro_def.sensitivity.as_str());
        let mut step_sensitivities: Vec<CommandSensitivityDto> = Vec::with_capacity(steps.len());
        for step in &steps {
            if let Ok(Some(def)) = self
                .services
                .command_service
                .get_command(GetCommandQuery {
                    actor: cmd.actor,
                    command_id: step.command_id.clone(),
                })
                .await
            {
                step_sensitivities.push(def.sensitivity);
            }
            // Unknown step command: skip sensitivity — dispatch will error at runtime.
        }
        let effective_sensitivity =
            CommandSensitivityDto::effective(macro_sensitivity, &step_sensitivities);

        // ── Verify the outer confirmation proof and issue per-step tokens.
        //
        // - Without a proof, return ConfirmationRequired with a challenge token
        //   the client can re-submit.
        // - With a verified proof, each step gets its own token derived from it,
        //   so a dangerous step's inner gate has a proof to consume.
        // - A step that still returns ConfirmationRequired is a hard failure
        //   (handled in the step loop below).
        let macro_cmd_id = format!("macro.{}", macro_def.id);
        // outer_proof_verified is true when the outer proof was verified against
        // the store (or when there is no store — test context).
        let outer_proof_verified = if effective_sensitivity.requires_confirmation_by_tier() {
            match &cmd.confirmation {
                None => {
                    // Issue a token so the client can re-submit with a proof.
                    let challenge =
                        self.services.confirmation_store.as_deref().map(|store| {
                            issue_in_band(store, &cmd.actor, &macro_cmd_id, &None, &None)
                        });
                    return Ok(CommandResultDto {
                        status: CommandResultStatusDto::ConfirmationRequired,
                        message: "macro requires confirmation due to sensitivity tier".to_string(),
                        payload: None,
                        follow_up_commands: vec![],
                        pipeline_stages: None,
                        confirmation_required: challenge,
                    });
                }
                Some(proof) => {
                    if let Some(store) = &self.services.confirmation_store {
                        // Proof is bound to (macro_cmd_id, no args, no context).
                        // Using macro_cmd_id prevents cross-macro replay.
                        verify_in_band(
                            store.as_ref(),
                            proof,
                            &cmd.actor,
                            &macro_cmd_id,
                            &None,
                            &None,
                        )
                        .map_err(|e| {
                            if e.is_stale() {
                                ServiceError::ConfirmationStale
                            } else {
                                ServiceError::PermissionDenied
                            }
                        })?;
                        true
                    } else {
                        // No store wired — proof cannot be verified. In
                        // production this should never happen (provider always
                        // wires the store); in test contexts it surfaces when
                        // a test supplies a proof without configuring a store.
                        tracing::warn!(
                            event = "macro_confirmation_store_missing",
                            macro_id = %macro_def.id,
                            "macro confirmation proof supplied but no confirmation_store wired; per-step verification will be skipped — production misconfiguration suspected"
                        );
                        false
                    }
                }
            }
        } else {
            false
        };

        let parent_audit_id = uuid::Uuid::now_v7();
        let server_id_for_audit: uuid::Uuid = macro_def.server_id.into();
        let initiator_str = cmd.initiator.as_audit_str();
        let sensitivity_str = effective_sensitivity.as_str();
        let confirmation_kind = if cmd.confirmation.is_some() {
            "in_band"
        } else {
            "none"
        };
        let dangerous = effective_sensitivity.is_mutating();

        let mut payload = serde_json::Map::new();
        let mut failed_at_step: Option<u32> = None;

        for (step_index, step) in steps.iter().enumerate() {
            // When the outer proof was verified, issue a per-step
            // in-band token bound to this exact (command_id, args, context) so
            // the inner confirmation gate has a valid proof to consume.
            let (step_confirmation, step_had_token) = if outer_proof_verified {
                match self.services.confirmation_store.as_deref().map(|store| {
                    let challenge = issue_in_band(
                        store,
                        &cmd.actor,
                        &step.command_id,
                        &step.args,
                        &cmd.command_context,
                    );
                    ConfirmationProof::InBandToken {
                        token: challenge.token,
                        args_hash: challenge.args_hash,
                    }
                }) {
                    Some(proof) => (Some(proof), true),
                    None => (None, false),
                }
            } else {
                (None, false)
            };

            let result = self
                .services
                .command_service
                .execute_command(ExecuteCommandCmd {
                    actor: cmd.actor,
                    command_id: step.command_id.clone(),
                    args: step.args.clone(),
                    context: cmd.command_context.clone(),
                    confirmation: step_confirmation,
                    is_pipe: false,
                    // Propagate outer initiator — Agent-triggered macros keep Agent
                    // on every nested call.
                    initiator: cmd.initiator,
                })
                .await;

            // Inner ConfirmationRequired is a hard failure — the step
            // did not execute. Treat it as MacroPartialFailure rather than Ok.
            let result = result.and_then(|r| {
                if r.status == CommandResultStatusDto::ConfirmationRequired {
                    Err(ServiceError::Internal(Some(anyhow::anyhow!(
                        "step returned ConfirmationRequired despite outer proof"
                    ))))
                } else {
                    Ok(r)
                }
            });

            // ── Emit a per-step child audit row carrying parent_audit_id.
            if let Some(audit_svc) = &self.services.audit_log_service {
                let child_entry = AuditLogEntryDto {
                    id: uuid::Uuid::now_v7(),
                    actor_user_id: cmd.actor.user_id.into(),
                    server_id: server_id_for_audit,
                    command_id: step.command_id.clone(),
                    initiator: initiator_str.to_string(),
                    args_redacted: serde_json::Value::Null,
                    sensitivity: step_sensitivities
                        .get(step_index)
                        .map(|s| s.as_str())
                        .unwrap_or("safe")
                        .to_string(),
                    dangerous,
                    confirmation_kind: if step_had_token { "in_band" } else { "none" }.to_string(),
                    typed_verb: None,
                    outcome: if result.is_ok() {
                        AuditOutcome::Success
                    } else {
                        AuditOutcome::StepError
                    },
                    error_kind: if result.is_err() {
                        Some(AuditOutcome::StepError.as_str().to_string())
                    } else {
                        None
                    },
                    capability_check_outcome: None,
                    confirmation_outcome: None,
                    parent_audit_id: Some(parent_audit_id),
                    created_at: chrono::Utc::now(),
                };
                if let Err(e) = audit_svc.write_entry(child_entry).await {
                    tracing::warn!(
                        event = "macro_step_audit_write_failed",
                        step = step_index,
                        error = %e,
                        "macro step audit row write failed"
                    );
                }
            }

            match result {
                Ok(r) => {
                    if let Some(sp) = r.payload {
                        payload.insert(step.command_id.clone(), sp);
                    }
                }
                Err(_) => {
                    failed_at_step = Some(step_index as u32);
                    break;
                }
            }
        }

        let (outcome, final_status) = if failed_at_step.is_some() {
            (
                AuditOutcome::MacroPartialFailure,
                CommandResultStatusDto::Error,
            )
        } else {
            (AuditOutcome::Success, CommandResultStatusDto::Success)
        };

        // Parent audit row written after all child rows so audit-log consumers
        // joining on parent_audit_id find the children already present.
        if let Some(audit_svc) = &self.services.audit_log_service {
            let parent_entry = AuditLogEntryDto {
                id: parent_audit_id,
                actor_user_id: cmd.actor.user_id.into(),
                server_id: server_id_for_audit,
                command_id: format!("macro.{}", macro_def.id),
                initiator: initiator_str.to_string(),
                args_redacted: serde_json::Value::Null,
                sensitivity: sensitivity_str.to_string(),
                dangerous,
                confirmation_kind: confirmation_kind.to_string(),
                typed_verb: None,
                outcome,
                error_kind: failed_at_step.map(|i| format!("step_error_at_{i}")),
                capability_check_outcome: None,
                confirmation_outcome: None,
                parent_audit_id: None,
                created_at: chrono::Utc::now(),
            };
            if let Err(e) = audit_svc.write_entry(parent_entry).await {
                tracing::warn!(
                    event = "macro_audit_write_failed",
                    error = %e,
                    "macro parent audit row write failed"
                );
            }
        }

        if failed_at_step.is_some() {
            return Ok(CommandResultDto {
                status: final_status,
                message: format!("macro `{}` failed on a step", macro_def.name),
                payload: Some(serde_json::Value::Object(payload)),
                follow_up_commands: vec![],
                pipeline_stages: None,
                confirmation_required: None,
            });
        }

        Ok(CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: format!("macro `{}` executed", macro_def.name),
            payload: Some(serde_json::Value::Object(payload)),
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        })
    }

    async fn synthesize_command_definitions(
        &self,
        query: SynthesizeMacroCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        let macros = self
            .repos
            .macro_repo
            .list_for_user(query.server_id, query.actor.user_id)
            .await?;

        let defs = macros
            .into_iter()
            .filter(|m| m.ai_callable)
            .map(|m| {
                let sensitivity = CommandSensitivityDto::from(m.sensitivity.as_str());
                let nl_desc = if m.nl_description.is_empty() {
                    m.description.clone().unwrap_or_default()
                } else {
                    m.nl_description.clone()
                };
                let intent_samples: Vec<String> = m.intent_samples.clone();
                CommandDefinitionDto {
                    id: format!("macro.{}", m.id),
                    title: m.name.clone(),
                    description: m.description.clone().unwrap_or_default(),
                    category: "Macro".to_string(),
                    source_kind: CommandSourceKindDto::User,
                    execution_target: CommandExecutionTargetDto::Backend,
                    arguments: vec![],
                    required_context: vec![],
                    required_capabilities: vec![],
                    visible_in_palette: false,
                    visible_in_context_menus: false,
                    keybind_capable: false,
                    dangerous: false,
                    user_command_id: None,
                    nl_description: Some(nl_desc),
                    intent_samples,
                    ai_callable: true,
                    sensitivity,
                    typed_verb: TypedVerb::Other,
                    rate_limit_per_minute: None,
                }
            })
            .collect();

        Ok(defs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::macro_def::model::{MacroDef, MacroId};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::command::sensitivity::CommandSensitivityDto;
    use rim_services::command::service::MockCommandService;

    use async_trait::async_trait;
    use rim_core::macro_def::repository::MacroRepository;
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::{ServerMember, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;

    mockall::mock! {
        ServerMemberRepository {}
        #[async_trait]
        impl ServerMemberRepository for ServerMemberRepository {
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    fn make_member_repo() -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo
    }

    mockall::mock! {
        MacroRepository {}
        #[async_trait]
        impl MacroRepository for MacroRepository {
            async fn get_by_id(&self, id: MacroId) -> RepoResult<Option<MacroDef>>;
            async fn find_by_name(
                &self,
                server_id: ServerId,
                user_id: UserId,
                name: &str,
            ) -> RepoResult<Option<MacroDef>>;
            async fn list_for_user(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<Vec<MacroDef>>;
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
                server_member_repo: Arc::new(make_member_repo()),
            },
            MacroServices {
                command_service: Arc::new(command_service),
                audit_log_service: None,
                confirmation_store: None,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
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
                ai_callable: new_macro.ai_callable,
                nl_description: new_macro.nl_description,
                intent_samples: new_macro.intent_samples,
                sensitivity: new_macro.sensitivity,
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "My Macro");
        assert_eq!(result.steps.len(), 1);
    }

    #[tokio::test]
    async fn create_and_list_refuse_a_non_member() {
        let mut member_repo = MockServerMemberRepository::new();
        member_repo.expect_is_member().returning(|_, _| Ok(false));
        member_repo
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));
        let svc = DefaultMacroService::new(
            MacroRepos {
                macro_repo: Arc::new(MockMacroRepository::new()),
                server_member_repo: Arc::new(member_repo),
            },
            MacroServices {
                command_service: Arc::new(MockCommandService::new()),
                audit_log_service: None,
                confirmation_store: None,
            },
        );

        let err = svc
            .create_macro(CreateMacroCmd {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
                name: "intrude".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));

        let err = svc
            .list_user_macros(ListUserMacrosQuery {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn execute_macro_refuses_anyone_but_the_author() {
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
                    { "commandId": "chat.post_message", "args": { "text": "hello" } }
                ]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });
        let svc = service(repo, MockCommandService::new());

        let err = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(UserId::new()),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
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
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
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
                ai_callable: true,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(2)
            .returning(|_| Box::pin(async { Ok(None) }));
        cmd_svc.expect_execute_command().times(2).returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                // Explicit Palette (not Default::default()) marks the
                // human-initiated path.
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn execute_macro_denies_agent_when_not_ai_callable() {
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
                    { "commandId": "chat.post_message", "args": { "text": "hello" } }
                ]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let cmd_svc = MockCommandService::new();
        let svc = service(repo, cmd_svc);

        let err = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Agent,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn execute_macro_allows_agent_when_ai_callable() {
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
                    { "commandId": "chat.post_message", "args": { "text": "hello" } }
                ]),
                ai_callable: true,
                nl_description: "Post a message in chat".to_string(),
                intent_samples: vec!["send a message".to_string()],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        cmd_svc.expect_execute_command().times(1).returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Agent,
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[test]
    fn sensitivity_dto_roundtrip_via_str() {
        for (s, expected) in [
            ("safe", CommandSensitivityDto::Safe),
            ("sensitive", CommandSensitivityDto::Sensitive),
            ("mutating", CommandSensitivityDto::Mutating),
            ("restricted", CommandSensitivityDto::Restricted),
        ] {
            let parsed = CommandSensitivityDto::from(s);
            assert_eq!(parsed, expected, "from({s:?}) should be {expected:?}");
            assert_eq!(parsed.as_str(), s, "as_str() roundtrip for {s:?}");
        }
        let unknown = CommandSensitivityDto::from("unknown_tier");
        assert_eq!(
            unknown,
            CommandSensitivityDto::Safe,
            "unknown str falls back to Safe"
        );
    }

    // ── Macro confirmation tests ─────────────────────────────────────────────

    /// A macro with Restricted sensitivity but no proof returns
    /// ConfirmationRequired with a challenge the client can re-submit.
    #[tokio::test]
    async fn execute_macro_restricted_no_proof_returns_challenge() {
        use rim_services::command::confirmation::shared_in_memory_store;

        let owner = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "dangerous macro".to_string(),
                description: None,
                steps_json: serde_json::json!([
                    { "commandId": "admin.nuke", "args": null }
                ]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "restricted".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));

        let store = shared_in_memory_store();
        let svc = DefaultMacroService::new(
            MacroRepos {
                macro_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo()),
            },
            MacroServices {
                command_service: Arc::new(cmd_svc),
                audit_log_service: None,
                confirmation_store: Some(Arc::clone(&store)),
            },
        );

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::ConfirmationRequired,
            "restricted macro without proof must return ConfirmationRequired"
        );
        assert!(
            result.confirmation_required.is_some(),
            "confirmation_required must be non-None — client needs the token to re-submit"
        );
    }

    /// A macro with a valid outer proof executes all steps and emits
    /// 3 audit rows (2 child + 1 parent).
    #[tokio::test]
    async fn execute_macro_valid_proof_executes_all_steps() {
        use rim_services::audit::service::MockAuditLogService;
        use rim_services::command::confirmation::{
            ConfirmationProof, issue_in_band, shared_in_memory_store,
        };

        let owner = UserId::new();
        let actor = Actor::new(owner);
        let macro_id = MacroId::new();
        let server_id = ServerId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id,
                user_id: owner,
                name: "my macro".to_string(),
                description: None,
                steps_json: serde_json::json!([
                    { "commandId": "cmd.alpha", "args": null },
                    { "commandId": "cmd.beta", "args": null }
                ]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "restricted".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        // Sensitivity lookups for 2 steps
        cmd_svc
            .expect_get_command()
            .times(2)
            .returning(|_| Box::pin(async { Ok(None) }));
        // Both steps execute successfully
        cmd_svc.expect_execute_command().times(2).returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        // 2 child rows + 1 parent row = 3 writes
        let mut audit_svc = MockAuditLogService::new();
        audit_svc
            .expect_write_entry()
            .times(3)
            .returning(|_| Box::pin(async { Ok(()) }));

        let store = shared_in_memory_store();
        let macro_cmd_id = format!("macro.{macro_id}");
        let challenge = issue_in_band(store.as_ref(), &actor, &macro_cmd_id, &None, &None);
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };

        let svc = DefaultMacroService::new(
            MacroRepos {
                macro_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo()),
            },
            MacroServices {
                command_service: Arc::new(cmd_svc),
                audit_log_service: Some(Arc::new(audit_svc)),
                confirmation_store: Some(Arc::clone(&store)),
            },
        );

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor,
                macro_id,
                command_context: None,
                confirmation: Some(proof),
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Success,
            "valid outer proof must let all steps execute: {result:?}"
        );
    }

    // ── Initiator propagation through macro dispatch ─────────────────────────
    //
    // Prove that the outer `CommandInitiator` is
    // preserved on every inner `execute_command` call that the macro step loop
    // issues. The `.withf` predicate on the mock expectation makes mockall panic
    // if the wrong initiator reaches the inner service — covering the scenario
    // where a future refactor silently resets the initiator to Palette before
    // calling `execute_command`.

    /// Outer initiator=Agent must propagate to every inner
    /// `execute_command` call. The `.withf` predicate proves the inner call
    /// carries `CommandInitiator::Agent`, not a defaulted Palette.
    #[tokio::test]
    async fn execute_macro_agent_initiator_propagates_to_inner_steps() {
        use rim_services::command::initiator::CommandInitiator;

        let owner = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "agent macro".to_string(),
                description: None,
                steps_json: serde_json::json!([
                    { "commandId": "chat.post_message", "args": { "text": "step1" } },
                    { "commandId": "chat.post_message", "args": { "text": "step2" } }
                ]),
                ai_callable: true,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(2)
            .returning(|_| Box::pin(async { Ok(None) }));
        // withf proves initiator=Agent reaches the inner service on every step.
        // If the macro loop reset initiator to Palette, mockall would panic here.
        cmd_svc
            .expect_execute_command()
            .withf(|cmd| cmd.initiator == CommandInitiator::Agent)
            .times(2)
            .returning(|_| {
                Box::pin(async {
                    Ok(CommandResultDto {
                        status: CommandResultStatusDto::Success,
                        message: "ok".to_string(),
                        payload: None,
                        follow_up_commands: vec![],
                        pipeline_stages: None,
                        confirmation_required: None,
                    })
                })
            });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: CommandInitiator::Agent,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Success,
            "macro must succeed — withf failure means wrong initiator reached inner step"
        );
    }

    /// Outer initiator=Palette must propagate to inner steps
    /// and must NOT trigger the agentic confirmation gate (no over-gating).
    #[tokio::test]
    async fn execute_macro_palette_initiator_propagates_to_inner_steps() {
        use rim_services::command::initiator::CommandInitiator;

        let owner = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "human macro".to_string(),
                description: None,
                steps_json: serde_json::json!([
                    { "commandId": "chat.post_message", "args": { "text": "step1" } }
                ]),
                ai_callable: true,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        // withf proves initiator=Palette (not Agent) reaches the inner service.
        cmd_svc
            .expect_execute_command()
            .withf(|cmd| cmd.initiator == CommandInitiator::Palette)
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(CommandResultDto {
                        status: CommandResultStatusDto::Success,
                        message: "ok".to_string(),
                        payload: None,
                        follow_up_commands: vec![],
                        pipeline_stages: None,
                        confirmation_required: None,
                    })
                })
            });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Success,
            "Palette macro must execute without confirmation challenge"
        );
    }

    /// An inner step returning ConfirmationRequired makes the macro a
    /// MacroPartialFailure (Error), NOT Success.
    #[tokio::test]
    async fn execute_macro_inner_confirmation_required_is_partial_failure() {
        let owner = UserId::new();
        let macro_id = MacroId::new();

        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(MacroDef {
                id: macro_id,
                server_id: ServerId::new(),
                user_id: owner,
                name: "test macro".to_string(),
                description: None,
                // Safe sensitivity so no outer confirmation is required;
                // outer_proof_verified stays false and step_confirmation is None.
                // This simulates a step that returns ConfirmationRequired due to
                // the inner service gating on a dangerous flag we can't intercept.
                steps_json: serde_json::json!([
                    { "commandId": "cmd.gated", "args": null }
                ]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        });

        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        // The inner step returns ConfirmationRequired
        cmd_svc.expect_execute_command().times(1).returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::ConfirmationRequired,
                    message: "confirm me".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        let svc = service(repo, cmd_svc);

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(owner),
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Error,
            "inner step returning ConfirmationRequired must be Error (MacroPartialFailure), not Success"
        );
    }
}
