use chrono::{DateTime, Utc};

use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::{
    Incident, IncidentId, IncidentMessageTag, IncidentPhaseId, IncidentPhaseName,
    IncidentPhaseRecord, IncidentSeverity, IncidentStatus, MessageTagType,
};
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IncidentTimelineDto {
    pub incident: IncidentDto,
    pub phases: Vec<IncidentPhaseDto>,
    pub tagged_messages: Vec<IncidentMessageTagDto>,
}
