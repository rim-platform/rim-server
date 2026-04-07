use crate::incident_phase;
use rim_core::incident::model::IncidentPhaseRecord;

impl From<incident_phase::Model> for IncidentPhaseRecord {
    fn from(m: incident_phase::Model) -> Self {
        Self {
            id: m.id.into(),
            incident_id: m.incident_id.into(),
            phase: m.phase.parse().unwrap_or(rim_core::incident::model::IncidentPhaseName::Detection),
            started_at: m.started_at.into(),
            ended_at: m.ended_at.map(Into::into),
            note: m.note,
        }
    }
}
