use crate::incident_message_tag;
use rim_core::incident::model::IncidentMessageTag;

impl From<incident_message_tag::Model> for IncidentMessageTag {
    fn from(m: incident_message_tag::Model) -> Self {
        Self {
            message_id: m.message_id.into(),
            incident_id: m.incident_id.into(),
            phase: m
                .phase
                .parse()
                .unwrap_or(rim_core::incident::model::IncidentPhaseName::Detection),
            tag_type: m.tag_type.and_then(|s| s.parse().ok()),
        }
    }
}
