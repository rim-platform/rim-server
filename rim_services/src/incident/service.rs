use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::escalation::model::EscalationRunId;
use rim_core::incident::model::IncidentId;
use rim_core::repository::pagination::Page;

use crate::ServiceResult;
use crate::escalation::dto::EscalationRunDto;
use crate::incident::command::{
    AcknowledgeIncidentCmd, AdvancePhaseCmd, EscalateIncidentNowCmd, GeneratePostmortemCmd,
    GetActiveIncidentQuery, GetIncidentTimelineQuery, IngestAlertCmd, IngestAlertOutcome,
    ListIncidentsQuery, ResolveIncidentCmd, StartIncidentCmd, TagMessageCmd,
    UpdateIncidentStatusCmd,
};
use crate::incident::dto::{
    IncidentDto, IncidentListPageCursor, IncidentMessageTagDto, IncidentPhaseDto,
    IncidentTimelineDto,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IncidentService: Send + Sync {
    async fn start_incident(&self, cmd: StartIncidentCmd) -> ServiceResult<IncidentDto>;

    async fn update_incident_status(
        &self,
        cmd: UpdateIncidentStatusCmd,
    ) -> ServiceResult<IncidentDto>;

    async fn advance_phase(&self, cmd: AdvancePhaseCmd) -> ServiceResult<IncidentPhaseDto>;

    async fn tag_message(&self, cmd: TagMessageCmd) -> ServiceResult<IncidentMessageTagDto>;

    async fn generate_postmortem(&self, cmd: GeneratePostmortemCmd) -> ServiceResult<IncidentDto>;

    async fn resolve_incident(&self, cmd: ResolveIncidentCmd) -> ServiceResult<IncidentDto>;

    async fn get_active_incident(
        &self,
        query: GetActiveIncidentQuery,
    ) -> ServiceResult<Option<IncidentDto>>;

    async fn get_incident_timeline(
        &self,
        query: GetIncidentTimelineQuery,
    ) -> ServiceResult<IncidentTimelineDto>;

    async fn list_incidents(
        &self,
        query: ListIncidentsQuery,
    ) -> ServiceResult<Page<IncidentDto, IncidentListPageCursor>>;

    /// Create-or-update-or-resolve an incident from a normalized alert (Track B2). See
    /// `docs/alerts.md` for the full dedup/severity/dispatch rules. Emits an
    /// `IncidentDeclared`/`IncidentUpdated` `WorkspaceEvent` on the underlying incident
    /// create/resolve (shared with `start_incident`/`resolve_incident`).
    async fn ingest_alert(&self, cmd: IngestAlertCmd) -> ServiceResult<IngestAlertOutcome>;

    /// Acknowledge an incident (§4). Idempotent; cancels the active escalation run, if any.
    /// Does not change `status`.
    async fn acknowledge_incident(&self, cmd: AcknowledgeIncidentCmd)
    -> ServiceResult<IncidentDto>;

    /// `incident.escalate` — fire the incident's current escalation step immediately (skipping
    /// the remaining wait), then re-arm/advance exactly as a normal timer fire would. No-op
    /// success if there is no active escalation run.
    async fn escalate_incident_now(&self, cmd: EscalateIncidentNowCmd) -> ServiceResult<()>;

    /// Called by the [`crate::escalation::scheduler::EscalationScheduler`] driver task when a
    /// run's timer fires (§5/§6). Reloads the run + incident and no-ops if the run is no longer
    /// `Active` (handles ack/resolve races per the at-least-once paging guarantee). Not part of
    /// any GraphQL surface — this is the executor side of the timer wheel.
    async fn fire_escalation_step(&self, run_id: EscalationRunId) -> ServiceResult<()>;

    /// A read projection of `incident_id`'s active escalation run, if any — backs `Incident.
    /// escalationRun` (§10 FLAG C) so a client loading mid-incident or after reconnect can render
    /// the live "escalating to L2 in 4:32" countdown (the `EscalationStepFired` event alone can't
    /// give `nextFireAt` to a fresh client). `None` when no policy is attached, or the run has
    /// already completed/been cancelled — only the currently-`Active` run is meaningful to show.
    /// Pure read: does not mutate any state, and — unlike every other method on this trait —
    /// takes no `Actor`, since the only caller is the `Incident` GraphQL resolver, which has
    /// already loaded (and therefore permission-gated) the parent `Incident` object; this method
    /// does not independently re-authorize.
    async fn get_active_escalation_run(
        &self,
        incident_id: IncidentId,
    ) -> ServiceResult<Option<EscalationRunDto>>;
}
