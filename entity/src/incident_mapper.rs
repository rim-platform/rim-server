use crate::incident;
use rim_core::incident::model::Incident;

impl From<incident::Model> for Incident {
    fn from(m: incident::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            conversation_id: m.conversation_id.into(),
            started_by: m.started_by.into(),
            status: m.status.parse().unwrap_or_else(|_| {
                tracing::warn!(incident_id = %m.id, status = %m.status, "unknown incident status, defaulting to Active");
                rim_core::incident::model::IncidentStatus::Active
            }),
            severity: m.severity.and_then(|s| s.parse().ok()),
            title: m.title,
            original_topic: m.original_topic,
            started_at: m.started_at.into(),
            mitigated_at: m.mitigated_at.map(Into::into),
            resolved_at: m.resolved_at.map(Into::into),
            postmortem_draft: m.postmortem_draft,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
