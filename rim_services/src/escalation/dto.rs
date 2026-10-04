use chrono::{DateTime, Utc};

use rim_core::escalation::model::{
    EscalationAction, EscalationPolicy, EscalationPolicyId, EscalationRun, EscalationRunId,
    EscalationRunStatus, EscalationStep, EscalationStepId, EscalationTarget,
};
use rim_core::incident::model::IncidentId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EscalationPolicyDto {
    pub id: EscalationPolicyId,
    pub server_id: ServerId,
    pub name: String,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Ordered by `position`. Populated by the service (a join the repository layer doesn't do
    /// on its own) — mirrors `RotationLayerDto::participants`.
    pub steps: Vec<EscalationStepDto>,
}

impl From<EscalationPolicy> for EscalationPolicyDto {
    fn from(p: EscalationPolicy) -> Self {
        Self {
            id: p.id,
            server_id: p.server_id,
            name: p.name,
            created_by: p.created_by,
            created_at: p.created_at,
            updated_at: p.updated_at,
            steps: Vec::new(),
        }
    }
}

/// Builds the policy-plus-steps shape [`crate::escalation::service::EscalationPolicyService`]
/// implementations resolve from `EscalationPolicy` + `EscalationStep` rows — mirrors
/// `rim_services::oncall::dto::rotation_layer_dto`.
pub fn escalation_policy_dto(
    policy: EscalationPolicy,
    steps: Vec<EscalationStep>,
) -> EscalationPolicyDto {
    EscalationPolicyDto {
        steps: steps.into_iter().map(EscalationStepDto::from).collect(),
        ..EscalationPolicyDto::from(policy)
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EscalationStepDto {
    pub id: EscalationStepId,
    pub policy_id: EscalationPolicyId,
    pub position: i32,
    pub delay_seconds: i64,
    pub target: EscalationTarget,
    pub actions: Vec<EscalationAction>,
}

impl From<EscalationStep> for EscalationStepDto {
    fn from(s: EscalationStep) -> Self {
        Self {
            id: s.id,
            policy_id: s.policy_id,
            position: s.position,
            delay_seconds: s.delay_seconds,
            target: s.target,
            actions: s.actions,
        }
    }
}

/// A step input for `createEscalationPolicy`/`updateEscalationPolicy` — no `id`/`policy_id`
/// (assigned by the repository) since the full step list is replaced atomically (§3).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EscalationStepInput {
    pub delay_seconds: i64,
    pub target: EscalationTarget,
    pub actions: Vec<EscalationAction>,
}

/// Read projection of the DB `escalation_run` row for a live incident (FLAG C —
/// `Incident.escalationRun`, landing in C1c's GraphQL resolver on top of this DTO).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EscalationRunDto {
    pub id: EscalationRunId,
    pub incident_id: IncidentId,
    pub policy_id: EscalationPolicyId,
    pub current_step_index: i32,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub status: EscalationRunStatus,
}

impl From<EscalationRun> for EscalationRunDto {
    fn from(r: EscalationRun) -> Self {
        Self {
            id: r.id,
            incident_id: r.incident_id,
            policy_id: r.policy_id,
            current_step_index: r.current_step_index,
            next_fire_at: r.next_fire_at,
            status: r.status,
        }
    }
}
