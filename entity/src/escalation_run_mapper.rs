use crate::escalation_run;
use rim_core::escalation::model::{EscalationRun, EscalationRunStatus};

impl From<escalation_run::Model> for EscalationRun {
    fn from(m: escalation_run::Model) -> Self {
        let run_id = m.id;
        let status = m.status.parse().unwrap_or_else(|_| {
            tracing::warn!(run_id = %run_id, status = %m.status, "unknown escalation run status, defaulting to Completed");
            EscalationRunStatus::Completed
        });

        Self {
            id: m.id.into(),
            incident_id: m.incident_id.into(),
            policy_id: m.policy_id.into(),
            current_step_index: m.current_step_index,
            next_fire_at: m.next_fire_at.map(Into::into),
            status,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
