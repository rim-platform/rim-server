use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::DatabaseConnection;

use rim_core::conversation::model::{Conversation, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::escalation::model::{
    EscalationAction, EscalationPolicyId, EscalationRunId, EscalationRunStatus, EscalationTarget,
    NewEscalationRun,
};
use rim_core::escalation::repository::{
    ConversationEscalationPolicyRepository, EscalationPolicyRepository, EscalationRunRepository,
    EscalationStepRepository,
};
use rim_core::incident::model::{
    Incident, IncidentPhaseName, IncidentSeverity, IncidentStatus, NewIncident,
    NewIncidentMessageTag,
};
use rim_core::incident::repository::{
    IncidentMessageTagRepository, IncidentPhaseRepository, IncidentRepository,
};
use rim_core::oncall::repository::{
    OnCallOverrideRepository, OnCallScheduleRepository, RotationLayerRepository,
    RotationParticipantRepository,
};
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::alert::AlertStatus;
use rim_services::escalation::scheduler::EscalationScheduler;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::incident::command::{
    AcknowledgeIncidentCmd, AdvancePhaseCmd, EscalateIncidentNowCmd, GeneratePostmortemCmd,
    GetActiveIncidentQuery, GetIncidentTimelineQuery, IngestAlertCmd, IngestAlertOutcome,
    ListIncidentsQuery, ResolveIncidentCmd, StartIncidentCmd, TagMessageCmd,
    UpdateIncidentStatusCmd,
};
use rim_services::incident::dto::{
    IncidentDto, IncidentListPageCursor, IncidentMessageTagDto, IncidentPhaseDto,
    IncidentTimelineDto,
};
use rim_services::incident::service::IncidentService;
use rim_services::message::command::SendMessageCmd;
use rim_services::message::service::MessageService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::oncall::{OnCallRepos, resolve_on_call_now_for_schedule};
use crate::repos::AppRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[derive(Clone)]
pub struct IncidentRepos {
    pub incident_repo: Arc<dyn IncidentRepository>,
    pub incident_phase_repo: Arc<dyn IncidentPhaseRepository>,
    pub incident_message_tag_repo: Arc<dyn IncidentMessageTagRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Used to resolve workspace owners' email addresses for the incident-declared
    /// notification (Track B3 §5).
    pub user_repo: Arc<dyn rim_core::user::repository::UserRepository>,
    /// Escalation run/policy/step state for arming, cancelling, and executing the escalation
    /// chain (C1b) — kept on `IncidentRepos` rather than a separate service dependency since
    /// declare/ack/resolve are already `IncidentService` methods that need to read/write this
    /// state directly (see `docs/oncall.md` "Escalation wiring").
    pub escalation_run_repo: Arc<dyn EscalationRunRepository>,
    pub escalation_policy_repo: Arc<dyn EscalationPolicyRepository>,
    pub escalation_step_repo: Arc<dyn EscalationStepRepository>,
    pub conversation_escalation_policy_repo: Arc<dyn ConversationEscalationPolicyRepository>,
    /// Used to resolve an `EscalationTarget::OnCall(schedule_id)` step target at fire time —
    /// see [`DefaultIncidentService::resolve_step_target`].
    pub on_call_schedule_repo: Arc<dyn OnCallScheduleRepository>,
    pub rotation_layer_repo: Arc<dyn RotationLayerRepository>,
    pub rotation_participant_repo: Arc<dyn RotationParticipantRepository>,
    pub on_call_override_repo: Arc<dyn OnCallOverrideRepository>,
}

pub struct DefaultIncidentService {
    repos: IncidentRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    email_sender: Arc<dyn crate::email::EmailSender>,
    /// e.g. `https://rim.example.com`. Used to build the incident-channel link in the
    /// incident-declared notification email. `None` means the notification is skipped entirely
    /// (no usable link to send) — logged at `warn` the first time this is hit per call, not
    /// spammed.
    public_base_url: Option<String>,
    /// Used to post the escalation-step mention message (§5, C1b). Threaded in directly at
    /// construction — this is the first place `IncidentService` needs it, so it wasn't wired in
    /// C1a (see `docs/oncall.md` "Escalation wiring" for the `started_by`-as-author decision).
    message_service: Arc<dyn MessageService>,
    /// Arms/cancels the escalation timer wheel (§6, C1b). The DB `escalation_run` row (via
    /// `IncidentRepos::escalation_run_repo`) is the source of truth; this is purely the timing
    /// mechanism the driver task in `rim_server::app` polls.
    escalation_scheduler: Arc<dyn EscalationScheduler>,
}

impl DefaultIncidentService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: IncidentRepos,
        db: Option<DatabaseConnection>,
        observers: Observers,
        email_sender: Arc<dyn crate::email::EmailSender>,
        public_base_url: Option<String>,
        message_service: Arc<dyn MessageService>,
        escalation_scheduler: Arc<dyn EscalationScheduler>,
    ) -> Self {
        Self {
            repos,
            db,
            observers,
            email_sender,
            public_base_url,
            message_service,
            escalation_scheduler,
        }
    }

    /// Email every workspace owner a "[SEVn] incident declared" notification (Track B3 §5).
    /// Best-effort: every failure (repo error, no owners, unconfigured `public_base_url`,
    /// send failure) is logged and swallowed — this must never fail incident creation, and it
    /// is a no-op-but-harmless "send" when email is unconfigured (`LogEmailSender`).
    ///
    /// Not gated on any notification preference — `notification_preference.rs` has no
    /// incident-email field yet; sending to all owners unconditionally is the documented
    /// interim behavior (see `docs/email.md` "Future work").
    async fn notify_owners_of_declared_incident(&self, incident: &Incident) {
        let Some(base_url) = &self.public_base_url else {
            tracing::warn!(
                incident_id = %incident.id,
                "RIM_PUBLIC_BASE_URL is not configured; skipping incident-declared email \
                 notification (no usable link to send)"
            );
            return;
        };

        let members = match self
            .repos
            .server_member_repo
            .get_members_for_server(incident.server_id)
            .await
        {
            Ok(members) => members,
            Err(e) => {
                tracing::warn!(error = %e, incident_id = %incident.id, "failed to list server members for incident-declared notification");
                return;
            }
        };

        let owner_ids: Vec<rim_core::user::model::UserId> = members
            .into_iter()
            .filter(|m| m.member_role == rim_core::server_member::model::ServerMemberRole::Owner)
            .map(|m| m.user_id)
            .collect();

        if owner_ids.is_empty() {
            tracing::warn!(incident_id = %incident.id, "no owners found for incident-declared notification");
            return;
        }

        let owners = match self.repos.user_repo.get_by_ids(&owner_ids).await {
            Ok(users) => users,
            Err(e) => {
                tracing::warn!(error = %e, incident_id = %incident.id, "failed to resolve owner emails for incident-declared notification");
                return;
            }
        };

        let severity_label = incident
            .severity
            .map(|s| s.as_str().to_ascii_uppercase())
            .unwrap_or_else(|| "unspecified".to_string());
        let link = format!(
            "{}/servers/{}/channels/{}",
            base_url.trim_end_matches('/'),
            incident.server_id,
            incident.conversation_id
        );

        for owner in owners {
            let message = crate::email::templates::incident_declared(
                &owner.email,
                &severity_label,
                &incident.title,
                &link,
            );
            if let Err(e) = self.email_sender.send(message).await {
                tracing::warn!(
                    error = %e,
                    incident_id = %incident.id,
                    owner_user_id = %owner.id,
                    "failed to send incident-declared email"
                );
            }
        }
    }

    async fn begin_write(&self) -> ServiceResult<(IncidentRepos, Option<TransactionScope>)> {
        if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let repos = AppRepos::with_handle(&scope.handle()).for_incident();
            Ok((repos, Some(scope)))
        } else {
            Ok((self.repos.clone(), None))
        }
    }

    async fn commit_if_present(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    // ── Escalation wiring (C1b) ─────────────────────────────────────────────
    //
    // See `docs/oncall.md` "Escalation wiring" for the full design. Summary: `start_incident`/
    // `ingest_alert` resolve a policy and create an `escalation_run` row *inside* the same
    // create transaction, but only call `EscalationScheduler::arm` *after* commit — a rollback
    // must never leave an in-memory timer armed with no matching DB row. `acknowledge_incident`/
    // `update_incident_status`(→Mitigated/Resolved)/`resolve_incident` cancel the run.

    /// Resolves the escalation policy to arm at declare time: an explicit `override_policy_id`
    /// wins, else the conversation's default (`conversation_escalation_policy`), else `None`
    /// (declared without an escalation chain — not an error, see §3).
    async fn resolve_declare_policy_id(
        repos: &IncidentRepos,
        conversation_id: rim_core::conversation::model::ConversationId,
        override_policy_id: Option<EscalationPolicyId>,
    ) -> ServiceResult<Option<EscalationPolicyId>> {
        if override_policy_id.is_some() {
            return Ok(override_policy_id);
        }
        Ok(repos
            .conversation_escalation_policy_repo
            .get(conversation_id)
            .await?)
    }

    /// Creates the `escalation_run` row for a freshly declared `incident` against `policy_id`,
    /// armed to fire step 0 at `incident.started_at + step[0].delay_seconds`. Every policy has
    /// at least one step (`EscalationPolicyService` validates this at authoring time), so
    /// `steps` is never empty here; if it somehow were, this is a no-op (logged) rather than a
    /// panic. Returns the `(run_id, fire_at)` to arm on the scheduler *after* the caller commits
    /// the transaction this ran in.
    async fn create_escalation_run_for_incident(
        repos: &IncidentRepos,
        incident: &Incident,
        policy_id: EscalationPolicyId,
    ) -> ServiceResult<Option<(EscalationRunId, chrono::DateTime<Utc>)>> {
        let steps = repos.escalation_step_repo.list_by_policy(policy_id).await?;
        let Some(first_step) = steps.first() else {
            tracing::warn!(
                incident_id = %incident.id,
                policy_id = %policy_id,
                "escalation policy has no steps; declaring incident without an escalation run"
            );
            return Ok(None);
        };
        let fire_at = incident.started_at + chrono::Duration::seconds(first_step.delay_seconds);
        let run = repos
            .escalation_run_repo
            .create(NewEscalationRun {
                incident_id: incident.id,
                policy_id,
                next_fire_at: Some(fire_at),
            })
            .await?;
        Ok(Some((run.id, fire_at)))
    }

    /// Cancels the incident's active escalation run, if any (ack/mitigate/resolve — §4/§6).
    /// Best-effort: a repo failure is logged and swallowed rather than failing the caller's
    /// lifecycle transition, mirroring `notify_owners_of_declared_incident`'s stance that
    /// escalation bookkeeping must never block the incident's primary state change.
    async fn cancel_escalation_run(&self, incident_id: rim_core::incident::model::IncidentId) {
        let run = match self
            .repos
            .escalation_run_repo
            .find_active_by_incident(incident_id)
            .await
        {
            Ok(run) => run,
            Err(e) => {
                tracing::warn!(error = %e, incident_id = %incident_id, "failed to look up active escalation run to cancel");
                return;
            }
        };
        let Some(run) = run else {
            return;
        };
        if let Err(e) = self
            .repos
            .escalation_run_repo
            .set_status(run.id, EscalationRunStatus::Cancelled)
            .await
        {
            tracing::warn!(error = %e, incident_id = %incident_id, run_id = %run.id, "failed to cancel escalation run");
            return;
        }
        self.escalation_scheduler.cancel(run.id);
    }

    /// Resolves an `EscalationTarget` to concrete user ids at fire time (§5 step 2).
    /// `User(u)` yields `[u]`; `OnCall(schedule)` resolves via
    /// [`resolve_on_call_now_for_schedule`] and yields 0 or 1 users — an empty/deleted schedule
    /// yields `[]` (log + skip actions, chain still advances).
    async fn resolve_step_target(
        repos: &IncidentRepos,
        target: EscalationTarget,
    ) -> ServiceResult<Vec<UserId>> {
        match target {
            EscalationTarget::User(user_id) => Ok(vec![user_id]),
            EscalationTarget::OnCall(schedule_id) => {
                let on_call_repos = OnCallRepos {
                    on_call_schedule_repo: Arc::clone(&repos.on_call_schedule_repo),
                    rotation_layer_repo: Arc::clone(&repos.rotation_layer_repo),
                    rotation_participant_repo: Arc::clone(&repos.rotation_participant_repo),
                    on_call_override_repo: Arc::clone(&repos.on_call_override_repo),
                    server_member_repo: Arc::clone(&repos.server_member_repo),
                };
                let resolved =
                    resolve_on_call_now_for_schedule(&on_call_repos, schedule_id, Utc::now())
                        .await?;
                Ok(resolved.into_iter().collect())
            }
        }
    }

    /// Page-time membership recheck (§5 step 2, C1 security review): drops any resolved target
    /// who is no longer a member of `server_id`, before any mention/email action is performed.
    /// This is the defense-in-depth backstop for
    /// `DefaultEscalationPolicyService::validate_steps`'s write-time constraint (a policy can
    /// only be *authored* to target same-server users/schedules) — it additionally closes the
    /// case where a target was a valid member when the step was authored (or last resolved via
    /// an on-call schedule/override) but has since left the server. A target dropped here is
    /// simply omitted from every downstream action (mention, email, the emitted
    /// `EscalationStepFired.target_user_ids`, and the timeline note); the chain still advances
    /// exactly as if that target had resolved to nobody (mirrors `perform_step_actions`'
    /// existing empty-targets no-op).
    async fn filter_current_members(
        repos: &IncidentRepos,
        server_id: ServerId,
        targets: Vec<UserId>,
    ) -> ServiceResult<Vec<UserId>> {
        let mut current = Vec::with_capacity(targets.len());
        for user_id in targets {
            if repos
                .server_member_repo
                .is_member(server_id, user_id)
                .await?
            {
                current.push(user_id);
            }
        }
        Ok(current)
    }

    /// Performs one step's actions best-effort (§5 step 3): a `MentionInChannel` posts a message
    /// authored as `incident.started_by` (§0.2 — the system-user follow-up, #70, is the long-term
    /// fix); an `Email` resolves each target's address and sends via the B3 sender. Every
    /// failure is logged at `warn` and does not abort the remaining actions or the step chain.
    async fn perform_step_actions(
        &self,
        incident: &Incident,
        step_index: i32,
        targets: &[UserId],
        actions: &[EscalationAction],
    ) {
        if targets.is_empty() {
            tracing::warn!(
                incident_id = %incident.id,
                step_index = step_index,
                "escalation step target resolved to no users; skipping actions, chain still advances"
            );
            return;
        }

        for action in actions {
            match action {
                EscalationAction::MentionInChannel => {
                    let text = targets
                        .iter()
                        .map(|u| format!("<@{u}>"))
                        .collect::<Vec<_>>()
                        .join(" ")
                        + &format!(
                            " — escalation step {} for incident \"{}\"",
                            step_index + 1,
                            incident.title
                        );
                    let cmd = SendMessageCmd {
                        actor: Actor::new(incident.started_by),
                        server_id: incident.server_id,
                        conversation_id: incident.conversation_id,
                        text: Some(text),
                        parent_message_id: None,
                        attachments: vec![],
                    };
                    if let Err(e) = self.message_service.send_message(cmd).await {
                        tracing::warn!(
                            error = %e,
                            incident_id = %incident.id,
                            step_index = step_index,
                            "failed to post escalation mention message (started_by may have left the server)"
                        );
                    }
                }
                EscalationAction::Email => {
                    let users = match self.repos.user_repo.get_by_ids(targets).await {
                        Ok(users) => users,
                        Err(e) => {
                            tracing::warn!(error = %e, incident_id = %incident.id, step_index = step_index, "failed to resolve escalation target emails");
                            continue;
                        }
                    };
                    let severity_label = incident
                        .severity
                        .map(|s| s.as_str().to_ascii_uppercase())
                        .unwrap_or_else(|| "unspecified".to_string());
                    let link = self.public_base_url.as_ref().map(|base_url| {
                        format!(
                            "{}/servers/{}/channels/{}",
                            base_url.trim_end_matches('/'),
                            incident.server_id,
                            incident.conversation_id
                        )
                    });
                    for user in users {
                        let message = crate::email::templates::escalation_step(
                            &user.email,
                            &severity_label,
                            &incident.title,
                            link.as_deref(),
                            step_index + 1,
                        );
                        if let Err(e) = self.email_sender.send(message).await {
                            tracing::warn!(
                                error = %e,
                                incident_id = %incident.id,
                                step_index = step_index,
                                target_user_id = %user.id,
                                "failed to send escalation-step email"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Channels have no `conversation_member` rows (server membership alone grants access), so
    /// the subscription-visibility list is only meaningful for DM/Group conversations. The
    /// `WorkspaceEvent::Incident*` variants carry `conversation_type` precisely so the
    /// subscriber-side filter doesn't need this list to be populated for channels.
    async fn member_user_ids_for_conversation(
        repos: &IncidentRepos,
        conversation: &Conversation,
    ) -> ServiceResult<Vec<UserId>> {
        if conversation.conversation_type == ConversationType::Channel {
            Ok(vec![])
        } else {
            Ok(repos
                .conversation_member_repo
                .list_member_ids(conversation.id)
                .await?)
        }
    }

    /// Emit `IncidentDeclared` for a newly created incident. Called from both `start_incident`
    /// (human-declared) and `ingest_alert` (alert-declared) so every incident creation
    /// broadcasts consistently — see `docs/alerts.md`.
    async fn emit_declared(&self, incident: &Incident, conversation: &Conversation) {
        let member_user_ids = match Self::member_user_ids_for_conversation(
            &self.repos,
            conversation,
        )
        .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e, incident_id = %incident.id, "failed to resolve incident conversation members for IncidentDeclared broadcast");
                vec![]
            }
        };
        self.observers.emit(WorkspaceEvent::IncidentDeclared {
            workspace_id: incident.server_id,
            conversation_id: incident.conversation_id,
            conversation_type: conversation.conversation_type.as_str().to_string(),
            incident_id: incident.id,
            severity: incident.severity.map(|s| s.as_str().to_string()),
            title: incident.title.clone(),
            member_user_ids,
        });

        self.notify_owners_of_declared_incident(incident).await;
    }

    /// Emit `IncidentUpdated` (status change or repeat-alert update). Shared by
    /// `resolve_incident` and `ingest_alert`'s update/resolve branches.
    async fn emit_updated(&self, incident: &Incident, conversation: &Conversation) {
        let member_user_ids = match Self::member_user_ids_for_conversation(
            &self.repos,
            conversation,
        )
        .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e, incident_id = %incident.id, "failed to resolve incident conversation members for IncidentUpdated broadcast");
                vec![]
            }
        };
        self.observers.emit(WorkspaceEvent::IncidentUpdated {
            workspace_id: incident.server_id,
            conversation_id: incident.conversation_id,
            conversation_type: conversation.conversation_type.as_str().to_string(),
            incident_id: incident.id,
            status: incident.status.as_str().to_string(),
            member_user_ids,
        });
    }
}

#[async_trait]
impl IncidentService for DefaultIncidentService {
    async fn start_incident(&self, cmd: StartIncidentCmd) -> ServiceResult<IncidentDto> {
        // Auth: caller must be a server member
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Validate conversation belongs to this server
        ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        // Validate title
        if cmd.title.is_empty() || cmd.title.chars().count() > 200 {
            return Err(ServiceError::Validation(
                "incident title must be between 1 and 200 characters".into(),
            ));
        }

        // Begin transaction for atomic create (all checks + writes in same scope)
        let (repos, scope) = self.begin_write().await?;

        // Check no active incident on this conversation (inside txn to prevent TOCTOU)
        let existing = repos
            .incident_repo
            .find_active_by_conversation(cmd.conversation_id)
            .await?;

        if existing.is_some() {
            return Err(ServiceError::Conflict(
                "an active incident already exists on this conversation".into(),
            ));
        }

        // Get current conversation topic to preserve for restoration
        let conversation = repos
            .conversation_repo
            .get_by_id(cmd.conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let original_topic = conversation.topic.clone();

        let escalation_policy_id =
            Self::resolve_declare_policy_id(&repos, cmd.conversation_id, cmd.escalation_policy_id)
                .await?;

        // Create the incident
        let incident = repos
            .incident_repo
            .create(NewIncident {
                server_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                started_by: cmd.actor.user_id,
                severity: cmd.severity,
                title: cmd.title.clone(),
                original_topic,
                alert_fingerprint: None,
                source: "manual".to_string(),
                escalation_policy_id,
            })
            .await?;

        // Create initial "detection" phase
        repos
            .incident_phase_repo
            .create(incident.id, IncidentPhaseName::Detection, None)
            .await?;

        // Update conversation topic to "[INCIDENT] {title}"
        let new_topic = format!("[INCIDENT] {}", cmd.title);
        repos
            .conversation_repo
            .update_topic(cmd.conversation_id, Some(new_topic))
            .await?;

        // Arm the escalation chain, if a policy applies (§6). The run row is created inside
        // this same transaction; the scheduler is only armed after commit succeeds below.
        let pending_arm = if let Some(policy_id) = escalation_policy_id {
            Self::create_escalation_run_for_incident(&repos, &incident, policy_id).await?
        } else {
            None
        };

        Self::commit_if_present(scope).await?;

        if let Some((run_id, fire_at)) = pending_arm {
            self.escalation_scheduler.arm(run_id, fire_at);
        }

        self.emit_declared(&incident, &conversation).await;

        Ok(IncidentDto::from(incident))
    }

    async fn update_incident_status(
        &self,
        cmd: UpdateIncidentStatusCmd,
    ) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Validate status transition
        match (&incident.status, &cmd.status) {
            (IncidentStatus::Active, IncidentStatus::Mitigated) => {}
            (IncidentStatus::Active, IncidentStatus::Resolved) => {}
            (IncidentStatus::Mitigated, IncidentStatus::Resolved) => {}
            (IncidentStatus::Mitigated, IncidentStatus::Active) => {}
            _ => {
                return Err(ServiceError::Validation(format!(
                    "cannot transition from {} to {}",
                    incident.status, cmd.status
                )));
            }
        }

        let now = Utc::now();
        let mitigated_at = match cmd.status {
            IncidentStatus::Mitigated => Some(now),
            _ => incident.mitigated_at,
        };
        let resolved_at = match cmd.status {
            IncidentStatus::Resolved => Some(now),
            _ => incident.resolved_at,
        };

        let updated = self
            .repos
            .incident_repo
            .update_status(cmd.incident_id, cmd.status, mitigated_at, resolved_at)
            .await?;

        // Reaching mitigation/resolution implies someone's handling it — cancel the escalation
        // chain (§4/§6). A reopen (Mitigated -> Active) does NOT re-arm a cancelled run.
        if matches!(
            cmd.status,
            IncidentStatus::Mitigated | IncidentStatus::Resolved
        ) {
            self.cancel_escalation_run(cmd.incident_id).await;
        }

        Ok(IncidentDto::from(updated))
    }

    async fn advance_phase(&self, cmd: AdvancePhaseCmd) -> ServiceResult<IncidentPhaseDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        if incident.status == IncidentStatus::Resolved {
            return Err(ServiceError::Validation(
                "cannot advance phase on a resolved incident".into(),
            ));
        }

        // Atomic: end current phase + create next phase
        let (repos, scope) = self.begin_write().await?;

        let current_phase = repos
            .incident_phase_repo
            .find_current_phase(cmd.incident_id)
            .await?;

        if let Some(current) = current_phase {
            repos.incident_phase_repo.end_phase(current.id).await?;
        }

        let new_phase = repos
            .incident_phase_repo
            .create(cmd.incident_id, cmd.next_phase, cmd.note)
            .await?;

        Self::commit_if_present(scope).await?;

        Ok(IncidentPhaseDto::from(new_phase))
    }

    async fn tag_message(&self, cmd: TagMessageCmd) -> ServiceResult<IncidentMessageTagDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Determine current phase
        let current_phase = self
            .repos
            .incident_phase_repo
            .find_current_phase(incident.id)
            .await?
            .map(|p| p.phase)
            .unwrap_or(IncidentPhaseName::Detection);

        let tag = self
            .repos
            .incident_message_tag_repo
            .create(NewIncidentMessageTag {
                message_id: cmd.message_id,
                incident_id: cmd.incident_id,
                phase: current_phase,
                tag_type: cmd.tag_type,
            })
            .await?;

        Ok(IncidentMessageTagDto::from(tag))
    }

    async fn generate_postmortem(&self, cmd: GeneratePostmortemCmd) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let phases = self
            .repos
            .incident_phase_repo
            .list_by_incident(incident.id)
            .await?;

        let tags = self
            .repos
            .incident_message_tag_repo
            .list_by_incident(incident.id)
            .await?;

        // Build template-based postmortem markdown
        let mut md = String::new();
        md.push_str(&format!("# Incident Postmortem: {}\n\n", incident.title));

        if let Some(sev) = &incident.severity {
            md.push_str(&format!("**Severity:** {}\n", sev));
        }
        md.push_str(&format!("**Started:** {}\n", incident.started_at));
        if let Some(at) = incident.mitigated_at {
            md.push_str(&format!("**Mitigated:** {}\n", at));
        }
        if let Some(at) = incident.resolved_at {
            md.push_str(&format!("**Resolved:** {}\n", at));
        }
        md.push('\n');

        for phase in &phases {
            md.push_str(&format!("## Phase: {}\n\n", phase.phase));
            md.push_str(&format!("- **Started:** {}\n", phase.started_at));
            if let Some(ended) = phase.ended_at {
                md.push_str(&format!("- **Ended:** {}\n", ended));
            }
            if let Some(note) = &phase.note {
                md.push_str(&format!("- **Note:** {}\n", note));
            }
            md.push('\n');

            // List tagged messages for this phase
            let phase_tags: Vec<_> = tags.iter().filter(|t| t.phase == phase.phase).collect();
            if !phase_tags.is_empty() {
                md.push_str("### Tagged Messages\n\n");
                for tag in phase_tags {
                    let tag_label = tag.tag_type.as_ref().map(|t| t.as_str()).unwrap_or("note");
                    md.push_str(&format!(
                        "- [{}] message_id: {}\n",
                        tag_label, tag.message_id
                    ));
                }
                md.push('\n');
            }
        }

        let updated = self
            .repos
            .incident_repo
            .update_postmortem_draft(incident.id, md)
            .await?;

        Ok(IncidentDto::from(updated))
    }

    async fn resolve_incident(&self, cmd: ResolveIncidentCmd) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        if incident.status == IncidentStatus::Resolved {
            return Err(ServiceError::Validation(
                "incident is already resolved".into(),
            ));
        }

        // Atomic: end phase + resolve + restore topic
        let (repos, scope) = self.begin_write().await?;

        // End current phase if any
        let current_phase = repos
            .incident_phase_repo
            .find_current_phase(incident.id)
            .await?;

        if let Some(current) = current_phase {
            repos.incident_phase_repo.end_phase(current.id).await?;
        }

        // Set status to resolved
        let now = Utc::now();
        let updated = repos
            .incident_repo
            .update_status(
                incident.id,
                IncidentStatus::Resolved,
                incident.mitigated_at,
                Some(now),
            )
            .await?;

        // Restore original conversation topic
        repos
            .conversation_repo
            .update_topic(incident.conversation_id, incident.original_topic.clone())
            .await?;

        Self::commit_if_present(scope).await?;

        self.cancel_escalation_run(incident.id).await;

        if let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(updated.conversation_id)
            .await?
        {
            self.emit_updated(&updated, &conversation).await;
        }

        Ok(IncidentDto::from(updated))
    }

    async fn get_active_incident(
        &self,
        query: GetActiveIncidentQuery,
    ) -> ServiceResult<Option<IncidentDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        // Validate conversation belongs to this server
        ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        let incident = self
            .repos
            .incident_repo
            .find_active_by_conversation(query.conversation_id)
            .await?;

        Ok(incident.map(IncidentDto::from))
    }

    async fn list_incidents(
        &self,
        query: ListIncidentsQuery,
    ) -> ServiceResult<Page<IncidentDto, IncidentListPageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let status_filter = query.status_filter.as_deref();
        let severity_filter = query.severity_filter.as_deref();

        let page = self
            .repos
            .incident_repo
            .list_incidents(
                query.server_id,
                status_filter,
                severity_filter,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
            )
            .await?;

        Ok(Page::new(
            page.items.into_iter().map(IncidentDto::from).collect(),
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn get_incident_timeline(
        &self,
        query: GetIncidentTimelineQuery,
    ) -> ServiceResult<IncidentTimelineDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(query.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            query.actor.user_id,
        )
        .await?;

        let phases = self
            .repos
            .incident_phase_repo
            .list_by_incident(incident.id)
            .await?;

        let tags = self
            .repos
            .incident_message_tag_repo
            .list_by_incident(incident.id)
            .await?;

        Ok(IncidentTimelineDto {
            incident: IncidentDto::from(incident),
            phases: phases.into_iter().map(IncidentPhaseDto::from).collect(),
            tagged_messages: tags.into_iter().map(IncidentMessageTagDto::from).collect(),
        })
    }

    async fn ingest_alert(&self, cmd: IngestAlertCmd) -> ServiceResult<IngestAlertOutcome> {
        // Auth: the token's created_by (== cmd.actor) must still be a server member; the
        // target conversation must still belong to this server. Both can go stale if the
        // token's creator leaves the server or the conversation is deleted after the token
        // was issued — the webhook handler surfaces that as a normal ServiceError.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        let (repos, scope) = self.begin_write().await?;

        let by_fingerprint = repos
            .incident_repo
            .find_by_fingerprint(cmd.server_id, &cmd.event.fingerprint)
            .await?;

        enum Effect {
            Ignored,
            Created(Incident),
            Updated(Incident),
            Resolved(Incident),
        }

        // Populated only by the `Created` branch below; armed on the scheduler after commit.
        let mut pending_arm: Option<(EscalationRunId, chrono::DateTime<Utc>)> = None;

        let effect = match (cmd.event.status, by_fingerprint) {
            (AlertStatus::Resolved, None) => Effect::Ignored,

            (AlertStatus::Resolved, Some(incident)) => {
                if let Some(current) = repos
                    .incident_phase_repo
                    .find_current_phase(incident.id)
                    .await?
                {
                    repos.incident_phase_repo.end_phase(current.id).await?;
                }
                let now = Utc::now();
                let updated = repos
                    .incident_repo
                    .update_status(
                        incident.id,
                        IncidentStatus::Resolved,
                        incident.mitigated_at,
                        Some(now),
                    )
                    .await?;
                repos
                    .conversation_repo
                    .update_topic(incident.conversation_id, incident.original_topic.clone())
                    .await?;
                Effect::Resolved(updated)
            }

            (AlertStatus::Firing, Some(incident)) => {
                // Same fingerprint, still open: record this firing as a timeline note rather
                // than creating a duplicate incident.
                let phase = repos
                    .incident_phase_repo
                    .find_current_phase(incident.id)
                    .await?
                    .map(|p| p.phase)
                    .unwrap_or(IncidentPhaseName::Detection);
                let note = cmd
                    .event
                    .summary
                    .clone()
                    .unwrap_or_else(|| format!("Alert re-fired: {}", cmd.event.title));
                repos
                    .incident_phase_repo
                    .create(incident.id, phase, Some(note))
                    .await?;
                Effect::Updated(incident)
            }

            (AlertStatus::Firing, None) => {
                // No incident tracks this fingerprint yet. If a *different* incident is
                // already active on the target conversation, don't create a second one on top
                // of it — attach a timeline note to the existing incident instead.
                if let Some(active) = repos
                    .incident_repo
                    .find_active_by_conversation(cmd.conversation_id)
                    .await?
                {
                    let note = format!(
                        "New alert detected (fingerprint {}): {}",
                        cmd.event.fingerprint, cmd.event.title
                    );
                    repos
                        .incident_phase_repo
                        .create(active.id, IncidentPhaseName::Detection, Some(note))
                        .await?;
                    Effect::Updated(active)
                } else {
                    let severity = cmd.event.severity.unwrap_or(IncidentSeverity::Sev3);
                    // No human to choose an override here — the channel default only (§3).
                    let escalation_policy_id = repos
                        .conversation_escalation_policy_repo
                        .get(cmd.conversation_id)
                        .await?;
                    let new = NewIncident {
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        started_by: cmd.actor.user_id,
                        severity: Some(severity),
                        title: cmd.event.title.clone(),
                        original_topic: conversation.topic.clone(),
                        alert_fingerprint: Some(cmd.event.fingerprint.clone()),
                        source: cmd.source.as_str().to_string(),
                        escalation_policy_id,
                    };
                    match repos.incident_repo.create(new).await {
                        Ok(incident) => {
                            repos
                                .incident_phase_repo
                                .create(
                                    incident.id,
                                    IncidentPhaseName::Detection,
                                    cmd.event.summary.clone(),
                                )
                                .await?;
                            let new_topic = format!("[INCIDENT] {}", cmd.event.title);
                            repos
                                .conversation_repo
                                .update_topic(cmd.conversation_id, Some(new_topic))
                                .await?;
                            if let Some(policy_id) = escalation_policy_id {
                                pending_arm = Self::create_escalation_run_for_incident(
                                    &repos, &incident, policy_id,
                                )
                                .await?;
                            }
                            Effect::Created(incident)
                        }
                        // The `idx_incident_active_fingerprint` partial unique index is the
                        // DB-level backstop for the find_by_fingerprint check above — a
                        // concurrent request can win the race between our check and this
                        // insert. Treat that race as "someone else just created it": re-fetch
                        // and attach our alert as a timeline note instead of erroring.
                        Err(e) if e.kind() == RepoErrorKind::Conflict => {
                            let winner = repos
                                .incident_repo
                                .find_by_fingerprint(cmd.server_id, &cmd.event.fingerprint)
                                .await?
                                .ok_or_else(|| {
                                    ServiceError::internal(anyhow::anyhow!(
                                        "incident create conflicted but no matching incident found by fingerprint"
                                    ))
                                })?;
                            let note =
                                cmd.event.summary.clone().unwrap_or_else(|| {
                                    format!("Alert re-fired: {}", cmd.event.title)
                                });
                            repos
                                .incident_phase_repo
                                .create(winner.id, IncidentPhaseName::Detection, Some(note))
                                .await?;
                            Effect::Updated(winner)
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        };

        Self::commit_if_present(scope).await?;

        if let Some((run_id, fire_at)) = pending_arm {
            self.escalation_scheduler.arm(run_id, fire_at);
        }

        Ok(match effect {
            Effect::Ignored => IngestAlertOutcome::Ignored,
            Effect::Created(incident) => {
                self.emit_declared(&incident, &conversation).await;
                IngestAlertOutcome::Created(IncidentDto::from(incident))
            }
            Effect::Updated(incident) => {
                self.emit_updated(&incident, &conversation).await;
                IngestAlertOutcome::Updated(IncidentDto::from(incident))
            }
            Effect::Resolved(incident) => {
                self.cancel_escalation_run(incident.id).await;
                self.emit_updated(&incident, &conversation).await;
                IngestAlertOutcome::Resolved(IncidentDto::from(incident))
            }
        })
    }

    async fn acknowledge_incident(
        &self,
        cmd: AcknowledgeIncidentCmd,
    ) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        // Any server member may ack — whoever responds picked it up (§4), not owner-gated.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Idempotent: acking an already-acked incident is a no-op success, not an error or a
        // re-page. Also covers acking a Resolved incident (a no-op-ish return per §4, since
        // resolve already cancelled the run and set no ack fields — acking post-resolution just
        // doesn't do anything further, which this branch also handles when acknowledged_at is
        // still None: it *would* set ack fields on a resolved incident, which is harmless (an
        // audit trail of "someone acked after resolution") and cancel_escalation_run is a no-op
        // since resolve already cancelled it).
        if incident.acknowledged_at.is_some() {
            return Ok(IncidentDto::from(incident));
        }

        let now = Utc::now();
        let updated = self
            .repos
            .incident_repo
            .acknowledge(cmd.incident_id, now, cmd.actor.user_id)
            .await?;

        self.cancel_escalation_run(cmd.incident_id).await;

        if let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(updated.conversation_id)
            .await?
        {
            let member_user_ids = Self::member_user_ids_for_conversation(&self.repos, &conversation)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, incident_id = %updated.id, "failed to resolve incident conversation members for IncidentAcknowledged broadcast");
                    vec![]
                });
            self.observers.emit(WorkspaceEvent::IncidentAcknowledged {
                workspace_id: updated.server_id,
                conversation_id: updated.conversation_id,
                conversation_type: conversation.conversation_type.as_str().to_string(),
                incident_id: updated.id,
                acknowledged_by: cmd.actor.user_id,
                member_user_ids,
            });
        }

        Ok(IncidentDto::from(updated))
    }

    async fn escalate_incident_now(&self, cmd: EscalateIncidentNowCmd) -> ServiceResult<()> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            incident.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let Some(run) = self
            .repos
            .escalation_run_repo
            .find_active_by_incident(cmd.incident_id)
            .await?
        else {
            // No active escalation run — a no-op success (nothing to escalate).
            return Ok(());
        };

        self.fire_escalation_step(run.id).await
    }

    async fn fire_escalation_step(&self, run_id: EscalationRunId) -> ServiceResult<()> {
        let Some(run) = self.repos.escalation_run_repo.find_by_id(run_id).await? else {
            tracing::warn!(run_id = %run_id, "fire_escalation_step: run not found, no-op");
            return Ok(());
        };
        // Handles ack/resolve races: the run may have been cancelled/completed between arming
        // and firing (§6 at-least-once guarantee — the reload-and-check IS the race guard).
        if run.status != EscalationRunStatus::Active {
            return Ok(());
        }

        let Some(incident) = self.repos.incident_repo.find_by_id(run.incident_id).await? else {
            tracing::warn!(run_id = %run_id, incident_id = %run.incident_id, "fire_escalation_step: incident not found, no-op");
            return Ok(());
        };

        let steps = self
            .repos
            .escalation_step_repo
            .list_by_policy(run.policy_id)
            .await?;
        let Some(step) = steps.get(run.current_step_index as usize) else {
            tracing::warn!(
                run_id = %run_id,
                policy_id = %run.policy_id,
                step_index = run.current_step_index,
                "fire_escalation_step: no step at current_step_index, completing run"
            );
            self.repos
                .escalation_run_repo
                .set_status(run.id, EscalationRunStatus::Completed)
                .await?;
            return Ok(());
        };

        let targets = Self::resolve_step_target(&self.repos, step.target).await?;
        let targets =
            Self::filter_current_members(&self.repos, incident.server_id, targets).await?;
        self.perform_step_actions(&incident, step.position, &targets, &step.actions)
            .await;

        // Timeline note (§5 step 4) — reuses the incident_phase mechanism, same as every other
        // timeline event in this service.
        let current_phase = self
            .repos
            .incident_phase_repo
            .find_current_phase(incident.id)
            .await?
            .map(|p| p.phase)
            .unwrap_or(IncidentPhaseName::Detection);
        let target_summary = if targets.is_empty() {
            "no target resolved".to_string()
        } else {
            targets
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Err(e) = self
            .repos
            .incident_phase_repo
            .create(
                incident.id,
                current_phase,
                Some(format!(
                    "Escalation step {} paged {} via {}",
                    step.position + 1,
                    target_summary,
                    step.actions
                        .iter()
                        .map(|a| a.as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                )),
            )
            .await
        {
            tracing::warn!(error = %e, incident_id = %incident.id, run_id = %run_id, "failed to append escalation-step timeline note");
        }

        // Advance the chain (§5 step 5).
        let next_index = run.current_step_index + 1;
        if let Some(next_step) = steps.get(next_index as usize) {
            let fire_at = Utc::now() + chrono::Duration::seconds(next_step.delay_seconds);
            self.repos
                .escalation_run_repo
                .advance(run.id, next_index, Some(fire_at))
                .await?;
            self.escalation_scheduler.arm(run.id, fire_at);
        } else {
            self.repos
                .escalation_run_repo
                .set_status(run.id, EscalationRunStatus::Completed)
                .await?;
        }

        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(incident.conversation_id)
            .await?;
        let (conversation_type, member_user_ids) = match &conversation {
            Some(c) => {
                let member_ids = Self::member_user_ids_for_conversation(&self.repos, c)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, incident_id = %incident.id, "failed to resolve incident conversation members for EscalationStepFired broadcast");
                        vec![]
                    });
                (c.conversation_type.as_str().to_string(), member_ids)
            }
            None => ("Channel".to_string(), vec![]),
        };

        self.observers.emit(WorkspaceEvent::EscalationStepFired {
            workspace_id: incident.server_id,
            conversation_id: incident.conversation_id,
            conversation_type,
            incident_id: incident.id,
            escalation_policy_id: run.policy_id,
            step_index: step.position,
            target_user_ids: targets,
            member_user_ids,
        });

        Ok(())
    }

    async fn get_active_escalation_run(
        &self,
        incident_id: rim_core::incident::model::IncidentId,
    ) -> ServiceResult<Option<rim_services::escalation::dto::EscalationRunDto>> {
        let run = self
            .repos
            .escalation_run_repo
            .find_active_by_incident(incident_id)
            .await?;
        Ok(run.map(rim_services::escalation::dto::EscalationRunDto::from))
    }
}
