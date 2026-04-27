use crate::Actor;
use crate::incident::dto::IncidentListPageCursor;
use rim_core::conversation::model::ConversationId;
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
