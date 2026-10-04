use crate::escalation_policy;
use rim_core::escalation::model::EscalationPolicy;

impl From<escalation_policy::Model> for EscalationPolicy {
    fn from(m: escalation_policy::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            name: m.name,
            created_by: m.created_by.into(),
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
