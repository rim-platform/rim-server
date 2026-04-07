use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::{
    IncidentId, IncidentPhaseName, IncidentSeverity, IncidentStatus, MessageTagType,
};
use rim_core::message::model::MessageId;
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
