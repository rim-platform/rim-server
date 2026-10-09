use chrono::{DateTime, Utc};

use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::{
    Incident, IncidentId, IncidentMessageTag, IncidentPhaseId, IncidentPhaseName,
    IncidentPhaseRecord, IncidentSeverity, IncidentStatus, MessageTagType,
};
use rim_core::incident::repository::IncidentListCursor;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct IncidentDto {
    pub id: IncidentId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub started_by: UserId,
    pub status: IncidentStatus,
    pub severity: Option<IncidentSeverity>,
    pub title: String,
    pub original_topic: Option<String>,
    pub started_at: DateTime<Utc>,
    pub mitigated_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub postmortem_draft: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub alert_fingerprint: Option<String>,
    pub source: String,
    pub escalation_policy_id: Option<rim_core::escalation::model::EscalationPolicyId>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<UserId>,
}

impl From<Incident> for IncidentDto {
    fn from(i: Incident) -> Self {
        Self {
            id: i.id,
            server_id: i.server_id,
            conversation_id: i.conversation_id,
            started_by: i.started_by,
            status: i.status,
            severity: i.severity,
            title: i.title,
            original_topic: i.original_topic,
            started_at: i.started_at,
            mitigated_at: i.mitigated_at,
            resolved_at: i.resolved_at,
            postmortem_draft: i.postmortem_draft,
            created_at: i.created_at,
            updated_at: i.updated_at,
            alert_fingerprint: i.alert_fingerprint,
            source: i.source,
            escalation_policy_id: i.escalation_policy_id,
            acknowledged_at: i.acknowledged_at,
            acknowledged_by: i.acknowledged_by,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncidentPhaseDto {
    pub id: IncidentPhaseId,
    pub incident_id: IncidentId,
    pub phase: IncidentPhaseName,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
}

impl From<IncidentPhaseRecord> for IncidentPhaseDto {
    fn from(p: IncidentPhaseRecord) -> Self {
        Self {
            id: p.id,
            incident_id: p.incident_id,
            phase: p.phase,
            started_at: p.started_at,
            ended_at: p.ended_at,
            note: p.note,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncidentMessageTagDto {
    pub message_id: MessageId,
    pub incident_id: IncidentId,
    pub phase: IncidentPhaseName,
    pub tag_type: Option<MessageTagType>,
}

impl From<IncidentMessageTag> for IncidentMessageTagDto {
    fn from(t: IncidentMessageTag) -> Self {
        Self {
            message_id: t.message_id,
            incident_id: t.incident_id,
            phase: t.phase,
            tag_type: t.tag_type,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncidentTimelineDto {
    pub incident: IncidentDto,
    pub phases: Vec<IncidentPhaseDto>,
    pub tagged_messages: Vec<IncidentMessageTagDto>,
}

/// Service-layer cursor for incident-list pagination.
///
/// Mirrors the repo cursor 1:1 — wrapped here so transport layers depend on
/// `rim_services` rather than reaching into `rim_core::incident::repository`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncidentListPageCursor {
    pub created_at: DateTime<Utc>,
    pub incident_id: IncidentId,
}

impl From<IncidentListCursor> for IncidentListPageCursor {
    fn from(value: IncidentListCursor) -> Self {
        Self {
            created_at: value.created_at,
            incident_id: value.incident_id,
        }
    }
}

impl From<IncidentListPageCursor> for IncidentListCursor {
    fn from(value: IncidentListPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            incident_id: value.incident_id,
        }
    }
}
