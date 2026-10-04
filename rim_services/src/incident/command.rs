use crate::Actor;
use crate::alert::{AlertEvent, AlertSource};
use crate::incident::dto::{IncidentDto, IncidentListPageCursor};
use rim_core::conversation::model::ConversationId;
use rim_core::escalation::model::EscalationPolicyId;
use rim_core::incident::model::{
    IncidentId, IncidentPhaseName, IncidentSeverity, IncidentStatus, MessageTagType,
};
use rim_core::message::model::MessageId;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct StartIncidentCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub title: String,
    pub severity: Option<IncidentSeverity>,
    /// Overrides the conversation's default escalation policy (`conversation_escalation_policy`)
    /// for this incident, if set. `None` falls back to the channel default; if neither exists,
    /// the incident is declared without an escalation chain (§3).
    pub escalation_policy_id: Option<EscalationPolicyId>,
}

#[derive(Debug, Clone)]
pub struct UpdateIncidentStatusCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    pub status: IncidentStatus,
}

#[derive(Debug, Clone)]
pub struct AdvancePhaseCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    pub next_phase: IncidentPhaseName,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TagMessageCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    pub message_id: MessageId,
    pub tag_type: Option<MessageTagType>,
}

#[derive(Debug, Clone)]
pub struct GeneratePostmortemCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
}

#[derive(Debug, Clone)]
pub struct ResolveIncidentCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
}

/// `acknowledgeIncident(incidentId)` / `incident.ack` (§4). Any server member may ack — not
/// owner-gated, since it means "whoever responds picked this up". Idempotent: acking an
/// already-acked incident is a no-op success, not an error.
#[derive(Debug, Clone)]
pub struct AcknowledgeIncidentCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    /// Command-dispatch initiator, when this call arrives via the command
    /// executor (C2 §3). `None` for direct GraphQL mutations. When
    /// `Some(i)` and `i.is_agentic()`, the service appends an
    /// agent-transcript timeline note to the incident.
    pub initiator: Option<crate::command::initiator::CommandInitiator>,
}

/// `incident.escalate` — manually trigger the incident's current escalation step immediately,
/// skipping the remaining wait. A no-op success if the incident has no active escalation run.
#[derive(Debug, Clone)]
pub struct EscalateIncidentNowCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    /// Command-dispatch initiator, when this call arrives via the command
    /// executor (C2 §3). `None` for direct GraphQL mutations. When
    /// `Some(i)` and `i.is_agentic()`, the service appends an
    /// agent-transcript timeline note to the incident.
    pub initiator: Option<crate::command::initiator::CommandInitiator>,
}

#[derive(Debug, Clone)]
pub struct GetActiveIncidentQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct GetIncidentTimelineQuery {
    pub actor: Actor,
    pub incident_id: IncidentId,
}

/// Query for `listIncidents` — server-scoped incident history with optional filters.
///
/// Within each filter array, values are OR-ed; the two filters are AND-ed.
/// `None`/empty arrays mean "no filtering on that field".
/// Ingest one normalized alert (Track B2). `actor` is always the ingest token's `created_by`
/// user (see `docs/alerts.md` "Actor model decision") — there is no unpersisted "webhook"
/// principal. `conversation_id` is the token's configured target channel.
#[derive(Debug, Clone)]
pub struct IngestAlertCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub source: AlertSource,
    pub event: AlertEvent,
}

/// Outcome of [`crate::incident::service::IncidentService::ingest_alert`], surfaced to the
/// webhook handler for the per-alert response summary.
#[derive(Debug, Clone)]
pub enum IngestAlertOutcome {
    /// A new incident was created for this fingerprint.
    Created(IncidentDto),
    /// An existing non-resolved incident for this fingerprint (or, if a different active
    /// incident already owns the target conversation, that incident) was updated.
    Updated(IncidentDto),
    /// A `resolved` alert matched an open incident, which is now resolved.
    Resolved(IncidentDto),
    /// A `resolved` alert had no matching open incident — nothing to do.
    Ignored,
}

#[derive(Debug, Clone)]
pub struct ListIncidentsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub status_filter: Option<Vec<IncidentStatus>>,
    pub severity_filter: Option<Vec<IncidentSeverity>>,
    pub dir: PageDir,
    pub cursor: Option<IncidentListPageCursor>,
    pub limit: u32,
}
