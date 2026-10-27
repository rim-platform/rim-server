use std::sync::Arc;

use async_trait::async_trait;

use rim_core::macro_def::model::{NewMacroDef, UpdateMacroDef};
use rim_core::macro_def::repository::MacroRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::bounds::{
    MAX_DESCRIPTION_CHARS, MAX_STEP_ARGS_BYTES, ensure_args_size, ensure_chars,
    ensure_command_id_size,
};
use rim_services::command::confirmation::{
    ConfirmationProof, ConfirmationStore, issue_in_band, verify_in_band,
};
use rim_services::command::dto::{
    CommandDefinitionDto, CommandExecutionTargetDto, CommandResultDto, CommandResultStatusDto,
    CommandSourceKindDto,
};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, GetCommandQuery};
use rim_services::command::sanitize::{MAX_INTENT_SAMPLE_LEN, MAX_NL_DESCRIPTION_LEN};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::service::CommandService;
use rim_services::command::verb::TypedVerb;
use rim_services::limits::{InstanceLimits, ensure_below, member_share};
use rim_services::macro_def::dto::{MacroDto, MacroStepDto};
use rim_services::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, GetMacroByNameQuery, ListUserMacrosQuery,
    SynthesizeMacroCommandsQuery, UpdateMacroCmd,
};
use rim_services::macro_def::service::MacroService;
use rim_services::{ServiceError, ServiceResult};

const MAX_STEPS: usize = 20;
const MAX_MACROS_PER_USER: u64 = 50;
/// Intent samples one macro may carry.
const MAX_INTENT_SAMPLES: usize = 10;
/// The refusal of a macro step naming a user command.
const USER_COMMAND_STEP: &str = "macro steps must not reference user commands";

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
    /// The cap on macros per workspace.
    limits: InstanceLimits,
    /// The server's default workspace, where no per-workspace cap applies and accounts in
    /// probation create none; `None` where none is wired.
    default_workspace: Option<crate::default_workspace_rules::DefaultWorkspaceRules>,
}

impl DefaultMacroService {
    pub fn new(repos: MacroRepos, services: MacroServices) -> Self {
        Self {
            repos,
            services,
            limits: InstanceLimits::default(),
            default_workspace: None,
        }
    }

    pub fn with_default_workspace(
        mut self,
        rules: crate::default_workspace_rules::DefaultWorkspaceRules,
    ) -> Self {
        self.default_workspace = Some(rules);
        self
    }

    /// Whether `server_id` is the default workspace, after refusing `user_id` there while in
    /// probation (`Validation` saying they can `action` once it ends).
    async fn in_default_workspace(
        &self,
        server_id: ServerId,
        user_id: UserId,
        action: &str,
    ) -> ServiceResult<bool> {
        let Some(rules) = &self.default_workspace else {
            return Ok(false);
        };
        if !rules.is_default(server_id).await? {
            return Ok(false);
        }
        rules.ensure_out_of_probation(user_id, action).await?;
        Ok(true)
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Why a run of a macro needs a confirmation proof, or `None` when it runs without one.
    ///
    /// A run needs one when the macro's own tier requires one by tier, or when a step's
    /// definition requires one from `initiator` under the command confirmation gate's rule
    /// (`CommandDefinitionDto::requires_confirmation_for_initiator`), the same rule that gate
    /// applies to the step when it runs. `step_definitions` pairs each resolved step's command
    /// id with its definition; the reason names each such step once, in step order.
    fn confirmation_reason(
        macro_sensitivity: CommandSensitivityDto,
        step_definitions: &[(String, CommandDefinitionDto)],
        initiator: CommandInitiator,
    ) -> Option<String> {
        let mut gated_steps: Vec<&str> = Vec::new();
        for (command_id, definition) in step_definitions {
            if definition.requires_confirmation_for_initiator(initiator)
                && !gated_steps.contains(&command_id.as_str())
            {
                gated_steps.push(command_id);
            }
        }
        let mut reasons: Vec<String> = Vec::new();
        if macro_sensitivity.requires_confirmation_by_tier() {
            reasons.push("sensitivity tier".to_string());
        }
        if !gated_steps.is_empty() {
            let noun = if gated_steps.len() == 1 {
                "step"
            } else {
                "steps"
            };
            let names: Vec<String> = gated_steps.iter().map(|id| format!("`{id}`")).collect();
            reasons.push(format!("{noun} {}", names.join(", ")));
        }
        if reasons.is_empty() {
            None
        } else {
            Some(format!(
                "macro requires confirmation due to {}",
                reasons.join(" and ")
            ))
        }
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

    /// Refuses `steps` when there are none or more than [`MAX_STEPS`], or when a step names a
    /// macro, a user command or no builtin command, has a command id longer than 256 bytes, or
    /// has arguments that serialize to more than [`MAX_STEP_ARGS_BYTES`]. A user command step is
    /// refused because each step runs with its own budget of builtin steps, which a user command
    /// could fill.
    ///
    /// # Errors
    ///
    /// `Validation` naming the reason.
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
            if step.command_id.starts_with("user.") {
                return Err(ServiceError::Validation(USER_COMMAND_STEP.to_string()));
            }
            ensure_command_id_size(&step.command_id, "a macro step's command id")?;
            if !crate::command::registry::is_builtin_command(&step.command_id) {
                return Err(ServiceError::Validation(format!(
                    "Unknown command: {}",
                    step.command_id
                )));
            }
            ensure_args_size(
                step.args.as_ref(),
                MAX_STEP_ARGS_BYTES,
                "a macro step's args",
            )?;
        }
        Ok(())
    }

    /// Refuses a description or natural-language description longer than
    /// [`MAX_DESCRIPTION_CHARS`] or [`MAX_NL_DESCRIPTION_LEN`] characters, or more than
    /// [`MAX_INTENT_SAMPLES`] intent samples or one longer than [`MAX_INTENT_SAMPLE_LEN`]
    /// characters.
    ///
    /// # Errors
    ///
    /// `Validation` naming the field.
    fn validate_text(
        description: Option<&str>,
        nl_description: &str,
        intent_samples: &[String],
    ) -> ServiceResult<()> {
        if let Some(description) = description {
            ensure_chars(description, MAX_DESCRIPTION_CHARS, "a macro description")?;
        }
        ensure_chars(
            nl_description,
            MAX_NL_DESCRIPTION_LEN,
            "a macro's natural-language description",
        )?;
        if intent_samples.len() > MAX_INTENT_SAMPLES {
            return Err(ServiceError::Validation(format!(
                "a macro may have at most {MAX_INTENT_SAMPLES} intent samples"
            )));
        }
        for sample in intent_samples {
            ensure_chars(sample, MAX_INTENT_SAMPLE_LEN, "a macro intent sample")?;
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
        Self::validate_text(
            cmd.description.as_deref(),
            &cmd.nl_description,
            &cmd.intent_samples,
        )?;

        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let in_default_workspace = self
            .in_default_workspace(cmd.server_id, cmd.actor.user_id, "create a macro")
            .await?;
        let is_moderator = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.actor.user_id)
            .await?
            .is_some_and(|m| m.member_role.is_moderator());
        let per_user = member_share(
            self.limits.macros_per_workspace,
            MAX_MACROS_PER_USER,
            is_moderator,
        );
        let count = self
            .repos
            .macro_repo
            .count_for_user(cmd.server_id, cmd.actor.user_id)
            .await?;
        if count >= per_user {
            return Err(ServiceError::Validation(format!(
                "user may have at most {per_user} macros per server"
            )));
        }
        // Macros belong to their author: in the default workspace the member share bounds
        // them and no account can fill a workspace cap for everyone.
        if !in_default_workspace && self.limits.macros_per_workspace.is_some() {
            let in_workspace = self
                .repos
                .macro_repo
                .count_for_server(cmd.server_id)
                .await?;
            ensure_below(
                self.limits.macros_per_workspace,
                in_workspace,
                "macros per workspace",
            )?;
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
        Self::validate_text(
            cmd.description.as_deref(),
            &cmd.nl_description,
            &cmd.intent_samples,
        )?;

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
        if steps
            .iter()
            .any(|step| step.command_id.starts_with("user."))
        {
            return Err(ServiceError::Validation(USER_COMMAND_STEP.to_string()));
        }

        // ── Resolve each step's definition via the command service. The effective
        // sensitivity is max(macro_tier, step_tiers), so a Safe-declared macro never runs below
        // a Restricted step; the definitions also decide whether the run needs a confirmation.
        let macro_sensitivity = CommandSensitivityDto::from(macro_def.sensitivity.as_str());
        let mut step_sensitivities: Vec<CommandSensitivityDto> = Vec::with_capacity(steps.len());
        let mut step_definitions: Vec<(String, CommandDefinitionDto)> =
            Vec::with_capacity(steps.len());
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
                step_definitions.push((step.command_id.clone(), def));
            }
            // Unknown step command: skip it here; dispatch fails on it at runtime.
        }
        let effective_sensitivity =
            CommandSensitivityDto::effective(macro_sensitivity, &step_sensitivities);
        let confirmation_reason =
            Self::confirmation_reason(macro_sensitivity, &step_definitions, cmd.initiator);

        // ── Verify the outer confirmation proof and issue per-step tokens.
        //
        // - A run that needs a confirmation (`confirmation_reason`) and has no proof returns
        //   ConfirmationRequired with a challenge token the client can re-submit.
        // - With a verified proof, each step gets its own token derived from it, so every step
        //   the inner gate stops has a proof to consume: one confirmation covers the run.
        // - A step that still returns ConfirmationRequired is a hard failure
        //   (handled in the step loop below).
        let macro_cmd_id = format!("macro.{}", macro_def.id);
        // outer_proof_verified is true when the outer proof was verified against
        // the store (or when there is no store — test context).
        let outer_proof_verified = if let Some(reason) = confirmation_reason {
            match &cmd.confirmation {
                None => {
                    // Issue a token so the client can re-submit with a proof.
                    let challenge =
                        self.services.confirmation_store.as_deref().map(|store| {
                            issue_in_band(store, &cmd.actor, &macro_cmd_id, &None, &None)
                        });
                    return Ok(CommandResultDto {
                        status: CommandResultStatusDto::ConfirmationRequired,
                        message: reason,
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
        let mut failure: Option<(u32, String)> = None;

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

            // A step fails when it errors or answers anything but Success; an inner
            // ConfirmationRequired is a failure too, since the step did not execute.
            let step_failure: Option<String> = match &result {
                Ok(r) => match r.status {
                    CommandResultStatusDto::Success => None,
                    CommandResultStatusDto::ConfirmationRequired => {
                        Some("the step needs a confirmation".to_string())
                    }
                    CommandResultStatusDto::Error => Some(r.message.clone()),
                },
                Err(e) => Some(step_error_reason(e)),
            };

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
                    outcome: if step_failure.is_none() {
                        AuditOutcome::Success
                    } else {
                        AuditOutcome::StepError
                    },
                    error_kind: if step_failure.is_some() {
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

            if let Some(reason) = step_failure {
                let title = step_definitions
                    .iter()
                    .find(|(id, _)| id == &step.command_id)
                    .map(|(_, d)| d.title.as_str())
                    .unwrap_or(step.command_id.as_str());
                failure = Some((
                    step_index as u32,
                    format!("Step {} ({}) failed: {}", step_index + 1, title, reason),
                ));
                break;
            }
            if let Ok(r) = result
                && let Some(sp) = r.payload
            {
                payload.insert(step.command_id.clone(), sp);
            }
        }

        let (outcome, final_status) = if failure.is_some() {
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
                error_kind: failure.as_ref().map(|(i, _)| format!("step_error_at_{i}")),
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

        if let Some((_, message)) = failure {
            return Ok(CommandResultDto {
                status: final_status,
                message,
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

/// The reason a failed step gives in the macro's answer: the message a caller of the
/// command alone would read, without a failed confirmation's detail.
fn step_error_reason(error: &ServiceError) -> String {
    match error {
        ServiceError::Validation(message)
        | ServiceError::Conflict(message)
        | ServiceError::Forbidden(message) => message.clone(),
        ServiceError::ConfirmationFailed(_) => "the step's confirmation failed".to_string(),
        other => other.to_string(),
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
            async fn count_members(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn get_members(&self, server_id: ServerId, user_ids: &[UserId]) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    fn make_member_repo() -> MockServerMemberRepository {
        member_repo_as(ServerMemberRole::Member)
    }

    /// A member repository where everyone is a member with `role`.
    fn member_repo_as(role: ServerMemberRole) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo.expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(Some(ServerMember {
                    id: rim_core::server_member::model::ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                }))
            });
        repo
    }

    mockall::mock! {
        MacroRepository {}
        #[async_trait]
        impl MacroRepository for MacroRepository {
            async fn count_for_server(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
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
            async fn delete_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;
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
    async fn create_macro_follows_the_macros_per_workspace_cap() {
        for (cap, in_workspace, allowed) in [
            (Some(50), Some(49), true),
            (Some(50), Some(50), false),
            (None, None, true),
        ] {
            let mut repo = MockMacroRepository::new();
            repo.expect_count_for_user().returning(|_, _| Ok(0));
            match in_workspace {
                Some(n) => {
                    repo.expect_count_for_server().returning(move |_| Ok(n));
                }
                None => {
                    repo.expect_count_for_server().never();
                }
            }
            repo.expect_create()
                .times(usize::from(allowed))
                .returning(|new_macro| {
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
            let svc = service(repo, MockCommandService::new()).with_limits(
                rim_services::limits::InstanceLimits {
                    macros_per_workspace: cap,
                    ..Default::default()
                },
            );
            let result = svc
                .create_macro(CreateMacroCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    name: "Capped".to_string(),
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
                .await;
            match result {
                Ok(_) => assert!(allowed, "{cap:?} {in_workspace:?}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {in_workspace:?}");
                    assert_eq!(message, "this instance allows 50 macros per workspace");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn the_default_workspace_takes_a_members_share_but_no_workspace_cap_or_probation() {
        let lobby = ServerId::new();
        let now = chrono::Utc::now();
        let (old, new) = (UserId::new(), UserId::new());
        let accounts = vec![
            crate::probation_fixtures::account(old, now - chrono::Duration::days(3), None),
            crate::probation_fixtures::account(new, now - chrono::Duration::hours(1), None),
        ];
        let limits = rim_services::limits::InstanceLimits {
            macros_per_workspace: Some(50),
            probation_hours: Some(24),
            ..Default::default()
        };
        for (actor, held, outcome) in [
            (old, 9, Ok(())),
            (old, 10, Err("user may have at most 10 macros per server")),
            (
                new,
                0,
                Err("a new account can create a macro in 1380 minutes"),
            ),
        ] {
            let mut repo = MockMacroRepository::new();
            repo.expect_count_for_user().returning(move |_, _| Ok(held));
            repo.expect_count_for_server().never();
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
            let svc = service(repo, MockCommandService::new())
                .with_limits(limits)
                .with_default_workspace(
                    crate::default_workspace_rules::DefaultWorkspaceRules::testing(
                        lobby,
                        accounts.clone(),
                        limits,
                        now,
                    ),
                );
            let result = svc
                .create_macro(CreateMacroCmd {
                    actor: Actor::new(actor),
                    server_id: lobby,
                    name: "Mine".to_string(),
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
                .await;
            match (result, outcome) {
                (Ok(_), Ok(())) => {}
                (Err(ServiceError::Validation(message)), Err(expected)) => {
                    assert_eq!(message, expected)
                }
                (other, expected) => panic!("expected {expected:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_member_holds_a_fifth_of_the_macros_per_workspace_cap() {
        for (role, held, allowed) in [
            (ServerMemberRole::Member, 9, true),
            (ServerMemberRole::Member, 10, false),
            (ServerMemberRole::Moderator, 10, true),
            (ServerMemberRole::Owner, 49, true),
        ] {
            let mut repo = MockMacroRepository::new();
            repo.expect_count_for_user().returning(move |_, _| Ok(held));
            repo.expect_count_for_server().returning(|_| Ok(0));
            repo.expect_create()
                .times(usize::from(allowed))
                .returning(|new_macro| {
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
            let svc = DefaultMacroService::new(
                MacroRepos {
                    macro_repo: Arc::new(repo),
                    server_member_repo: Arc::new(member_repo_as(role)),
                },
                MacroServices {
                    command_service: Arc::new(MockCommandService::new()),
                    audit_log_service: None,
                    confirmation_store: None,
                },
            )
            .with_limits(rim_services::limits::InstanceLimits {
                macros_per_workspace: Some(50),
                ..Default::default()
            });
            let result = svc
                .create_macro(CreateMacroCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    name: "Shared".to_string(),
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
                .await;
            match result {
                Ok(_) => assert!(allowed, "{role:?} {held}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{role:?} {held}");
                    assert_eq!(message, "user may have at most 10 macros per server");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
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
    // where the initiator is silently reset to Palette before calling
    // `execute_command`.

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
                // the inner service gating on a dangerous flag the macro can't intercept.
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
        assert_eq!(
            result.message,
            "Step 1 (cmd.gated) failed: the step needs a confirmation"
        );
    }

    // ── Up-front confirmation rule ───────────────────────────────────────────

    fn step_definition(
        id: &str,
        dangerous: bool,
        sensitivity: CommandSensitivityDto,
    ) -> (String, CommandDefinitionDto) {
        (
            id.to_string(),
            CommandDefinitionDto {
                id: id.to_string(),
                title: id.to_string(),
                description: String::new(),
                category: "Test".to_string(),
                source_kind: CommandSourceKindDto::BuiltIn,
                execution_target: CommandExecutionTargetDto::Backend,
                arguments: vec![],
                required_context: vec![],
                required_capabilities: vec![],
                visible_in_palette: true,
                visible_in_context_menus: false,
                keybind_capable: true,
                dangerous,
                user_command_id: None,
                nl_description: None,
                intent_samples: vec![],
                ai_callable: false,
                sensitivity,
                typed_verb: TypedVerb::Other,
                rate_limit_per_minute: None,
            },
        )
    }

    /// A dangerous `Mutating` step makes a palette run ask for a confirmation, and the reason
    /// names it; a `Restricted` step is named the same way.
    #[test]
    fn confirmation_reason_names_dangerous_and_restricted_steps() {
        let steps = [
            step_definition("chat.list_channels", false, CommandSensitivityDto::Safe),
            step_definition("admin.kick", true, CommandSensitivityDto::Mutating),
            step_definition("chat.post_message", false, CommandSensitivityDto::Mutating),
            step_definition("admin.kick", true, CommandSensitivityDto::Mutating),
            step_definition(
                "chat.delete_channel",
                false,
                CommandSensitivityDto::Restricted,
            ),
        ];
        let reason = DefaultMacroService::confirmation_reason(
            CommandSensitivityDto::Safe,
            &steps,
            CommandInitiator::Palette,
        );
        assert_eq!(
            reason.as_deref(),
            Some("macro requires confirmation due to steps `admin.kick`, `chat.delete_channel`")
        );
    }

    /// The macro's own `Restricted` tier asks for a confirmation even when no step does.
    #[test]
    fn confirmation_reason_covers_the_macros_own_tier() {
        let steps = [step_definition(
            "chat.post_message",
            false,
            CommandSensitivityDto::Mutating,
        )];
        assert_eq!(
            DefaultMacroService::confirmation_reason(
                CommandSensitivityDto::Restricted,
                &steps,
                CommandInitiator::Palette,
            )
            .as_deref(),
            Some("macro requires confirmation due to sensitivity tier")
        );
        let dangerous = [step_definition(
            "admin.ban",
            true,
            CommandSensitivityDto::Mutating,
        )];
        assert_eq!(
            DefaultMacroService::confirmation_reason(
                CommandSensitivityDto::Restricted,
                &dangerous,
                CommandInitiator::Palette,
            )
            .as_deref(),
            Some("macro requires confirmation due to sensitivity tier and step `admin.ban`")
        );
    }

    /// Without a dangerous or `Restricted` step a human-started run needs no confirmation,
    /// while the same `Mutating` step started by an agent does, as the step gate decides.
    #[test]
    fn confirmation_reason_follows_the_step_gate_for_each_initiator() {
        let steps = [
            step_definition("chat.list_channels", false, CommandSensitivityDto::Safe),
            step_definition("chat.post_message", false, CommandSensitivityDto::Mutating),
        ];
        for initiator in [
            CommandInitiator::Palette,
            CommandInitiator::Keybind,
            CommandInitiator::Macro,
        ] {
            assert_eq!(
                DefaultMacroService::confirmation_reason(
                    CommandSensitivityDto::Safe,
                    &steps,
                    initiator,
                ),
                None,
                "{initiator:?}"
            );
        }
        assert_eq!(
            DefaultMacroService::confirmation_reason(
                CommandSensitivityDto::Safe,
                &steps,
                CommandInitiator::Agent,
            )
            .as_deref(),
            Some("macro requires confirmation due to step `chat.post_message`")
        );
        for (id, def) in &steps {
            let gate_stops = def.requires_confirmation_for_initiator(CommandInitiator::Agent);
            let one = [(id.clone(), def.clone())];
            assert_eq!(
                DefaultMacroService::confirmation_reason(
                    CommandSensitivityDto::Safe,
                    &one,
                    CommandInitiator::Agent,
                )
                .is_some(),
                gate_stops,
                "{id}"
            );
        }
    }

    fn macro_cmd(steps: Vec<MacroStepDto>) -> CreateMacroCmd {
        CreateMacroCmd {
            actor: Actor::new(UserId::new()),
            server_id: ServerId::new(),
            name: "Bounded".to_string(),
            description: None,
            steps,
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
            sensitivity: CommandSensitivityDto::Safe,
        }
    }

    fn post_step(args: Option<serde_json::Value>) -> MacroStepDto {
        MacroStepDto {
            command_id: "chat.post_message".to_string(),
            args,
        }
    }

    #[tokio::test]
    async fn create_macro_refuses_user_command_steps_and_oversized_fields() {
        let svc = service(MockMacroRepository::new(), MockCommandService::new());
        let user_step = MacroStepDto {
            command_id: "user.many_steps".to_string(),
            args: None,
        };
        let cases = [
            (
                macro_cmd(vec![post_step(None), user_step]),
                USER_COMMAND_STEP,
            ),
            (
                macro_cmd(vec![post_step(Some(
                    serde_json::json!({ "text": "x".repeat(4096) }),
                ))]),
                "a macro step's args may be at most 4096 bytes",
            ),
            (
                macro_cmd(vec![MacroStepDto {
                    command_id: format!("chat.{}", "x".repeat(300)),
                    args: None,
                }]),
                "a macro step's command id may be at most 256 bytes",
            ),
            (
                CreateMacroCmd {
                    description: Some("d".repeat(1001)),
                    ..macro_cmd(vec![post_step(None)])
                },
                "a macro description may be at most 1000 characters",
            ),
            (
                CreateMacroCmd {
                    nl_description: "n".repeat(1001),
                    ..macro_cmd(vec![post_step(None)])
                },
                "a macro's natural-language description may be at most 1000 characters",
            ),
            (
                CreateMacroCmd {
                    intent_samples: vec!["post it".to_string(); 11],
                    ..macro_cmd(vec![post_step(None)])
                },
                "a macro may have at most 10 intent samples",
            ),
            (
                CreateMacroCmd {
                    intent_samples: vec!["s".repeat(201)],
                    ..macro_cmd(vec![post_step(None)])
                },
                "a macro intent sample may be at most 200 characters",
            ),
        ];
        for (cmd, refusal) in cases {
            let err = svc.create_macro(cmd).await.unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == refusal),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn update_macro_refuses_user_command_steps() {
        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([{ "commandId": "chat.post_message", "args": null }]),
            )))
        });
        repo.expect_update().never();
        let svc = service(repo, MockCommandService::new());

        let err = svc
            .update_macro(UpdateMacroCmd {
                actor,
                macro_id,
                name: "Bounded".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "user.many_steps".to_string(),
                    args: None,
                }],
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(&err, ServiceError::Validation(m) if m == USER_COMMAND_STEP),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn execute_macro_refuses_a_stored_user_command_step_before_any_step_runs() {
        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([
                    { "commandId": "chat.post_message", "args": null },
                    { "commandId": "user.many_steps", "args": null }
                ]),
            )))
        });
        let mut cmd_svc = MockCommandService::new();
        cmd_svc.expect_get_command().never();
        cmd_svc.expect_execute_command().never();
        let svc = service(repo, cmd_svc);

        let err = svc
            .execute_macro(ExecuteMacroCmd {
                actor,
                macro_id,
                command_context: None,
                confirmation: None,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(&err, ServiceError::Validation(m) if m == USER_COMMAND_STEP),
            "{err:?}"
        );
    }

    /// A command service whose `get_command` answers a definition titled "Post Message".
    fn post_message_definition(cmd_svc: &mut MockCommandService) {
        cmd_svc.expect_get_command().returning(|query| {
            let (_, mut definition) =
                step_definition(&query.command_id, false, CommandSensitivityDto::Mutating);
            definition.title = "Post Message".to_string();
            Box::pin(async move { Ok(Some(definition)) })
        });
    }

    fn run_macro(actor: Actor, macro_id: MacroId) -> ExecuteMacroCmd {
        ExecuteMacroCmd {
            actor,
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: rim_services::command::initiator::CommandInitiator::Palette,
        }
    }

    fn step_result(status: CommandResultStatusDto, message: &str) -> CommandResultDto {
        CommandResultDto {
            status,
            message: message.to_string(),
            payload: None,
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        }
    }

    #[tokio::test]
    async fn execute_macro_names_the_failing_step_and_its_reason() {
        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([
                    { "commandId": "chat.post_message", "args": { "text": "hello" } },
                    { "commandId": "chat.post_message", "args": null }
                ]),
            )))
        });
        let mut cmd_svc = MockCommandService::new();
        post_message_definition(&mut cmd_svc);
        let mut calls = 0;
        cmd_svc
            .expect_execute_command()
            .times(2)
            .returning(move |_| {
                calls += 1;
                let result = if calls == 1 {
                    Ok(step_result(CommandResultStatusDto::Success, "ok"))
                } else {
                    Err(ServiceError::Validation(
                        "missing string arg `text`".to_string(),
                    ))
                };
                Box::pin(async move { result })
            });
        let svc = service(repo, cmd_svc);

        let result = svc.execute_macro(run_macro(actor, macro_id)).await.unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Error);
        assert_eq!(
            result.message,
            "Step 2 (Post Message) failed: missing string arg `text`"
        );
    }

    #[tokio::test]
    async fn execute_macro_names_an_unknown_step_by_its_id() {
        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([{ "commandId": "chat.gone", "args": null }]),
            )))
        });
        let mut cmd_svc = MockCommandService::new();
        cmd_svc
            .expect_get_command()
            .returning(|_| Box::pin(async { Ok(None) }));
        cmd_svc
            .expect_execute_command()
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let svc = service(repo, cmd_svc);

        let result = svc.execute_macro(run_macro(actor, macro_id)).await.unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Error);
        assert_eq!(result.message, "Step 1 (chat.gone) failed: not found");
    }

    #[tokio::test]
    async fn execute_macro_stops_at_a_step_that_answers_error() {
        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([
                    { "commandId": "chat.post_message", "args": { "text": "one" } },
                    { "commandId": "chat.post_message", "args": { "text": "two" } }
                ]),
            )))
        });
        let mut cmd_svc = MockCommandService::new();
        post_message_definition(&mut cmd_svc);
        cmd_svc.expect_execute_command().times(1).returning(|_| {
            Box::pin(async { Ok(step_result(CommandResultStatusDto::Error, "boom")) })
        });
        let svc = service(repo, cmd_svc);

        let result = svc.execute_macro(run_macro(actor, macro_id)).await.unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Error);
        assert_eq!(result.message, "Step 1 (Post Message) failed: boom");
    }

    #[test]
    fn a_step_error_reads_as_the_command_alone_would_say_it() {
        assert_eq!(
            step_error_reason(&ServiceError::Validation(
                "missing string arg `text`".into()
            )),
            "missing string arg `text`"
        );
        assert_eq!(
            step_error_reason(&ServiceError::ConfirmationFailed("wrong actor".into())),
            "the step's confirmation failed"
        );
        assert_eq!(step_error_reason(&ServiceError::NotFound), "not found");
    }

    #[tokio::test]
    async fn create_and_update_macro_refuse_an_unknown_command() {
        let svc = service(MockMacroRepository::new(), MockCommandService::new());
        let err = svc
            .create_macro(macro_cmd(vec![
                post_step(None),
                MacroStepDto {
                    command_id: "chat.nope".to_string(),
                    args: None,
                },
            ]))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "Unknown command: chat.nope"),
            "{err:?}"
        );

        let actor = Actor::new(UserId::new());
        let macro_id = MacroId::new();
        let mut repo = MockMacroRepository::new();
        repo.expect_get_by_id().returning(move |_| {
            Ok(Some(stored_macro(
                macro_id,
                actor.user_id,
                serde_json::json!([{ "commandId": "chat.post_message", "args": null }]),
            )))
        });
        let svc = service(repo, MockCommandService::new());
        let err = svc
            .update_macro(UpdateMacroCmd {
                actor,
                macro_id,
                name: "Bounded".to_string(),
                description: None,
                steps: vec![MacroStepDto {
                    command_id: "plugin.dev.example.x".to_string(),
                    args: None,
                }],
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: CommandSensitivityDto::Safe,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "Unknown command: plugin.dev.example.x"),
            "{err:?}"
        );
    }

    fn stored_macro(id: MacroId, user_id: UserId, steps_json: serde_json::Value) -> MacroDef {
        MacroDef {
            id,
            server_id: ServerId::new(),
            user_id,
            name: "Bounded".to_string(),
            description: None,
            steps_json,
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
            sensitivity: "safe".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }
}
