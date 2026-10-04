use crate::escalation_step;
use rim_core::escalation::model::{
    EscalationStep, EscalationTarget, EscalationTargetKindTag, decode_actions,
};
use rim_core::user::model::UserId;

/// Fallback used when a stored per-kind target field is unexpectedly missing/invalid (should
/// never happen for rows written by this service — the DB CHECK `chk_escalation_step_target`
/// and the write path both enforce exactly one of `target_user_id`/`target_schedule_id` per
/// `target_kind` — but a pure `From` impl must not panic on unexpected DB state). Falls back to
/// a nil-UUID user target, mirroring `on_call_rotation_layer_mapper`'s fallback style.
fn fallback_user_target(step_id: sea_orm::prelude::Uuid) -> EscalationTarget {
    tracing::warn!(step_id = %step_id, "escalation step target user id missing/invalid, defaulting to nil user");
    EscalationTarget::User(UserId::from(sea_orm::prelude::Uuid::nil()))
}

impl From<escalation_step::Model> for EscalationStep {
    fn from(m: escalation_step::Model) -> Self {
        let step_id = m.id;
        let tag = m.target_kind.parse().unwrap_or_else(|_| {
            tracing::warn!(step_id = %step_id, target_kind = %m.target_kind, "unknown escalation target kind, defaulting to User");
            EscalationTargetKindTag::User
        });

        let target = match tag {
            EscalationTargetKindTag::User => match m.target_user_id {
                Some(uid) => EscalationTarget::User(uid.into()),
                None => fallback_user_target(step_id),
            },
            EscalationTargetKindTag::OnCall => match m.target_schedule_id {
                Some(sid) => EscalationTarget::OnCall(sid.into()),
                None => fallback_user_target(step_id),
            },
        };

        let actions = decode_actions(&m.actions).unwrap_or_else(|e| {
            tracing::warn!(step_id = %step_id, actions = %m.actions, error = %e, "failed to decode escalation step actions, defaulting to empty");
            Vec::new()
        });

        Self {
            id: m.id.into(),
            policy_id: m.policy_id.into(),
            position: m.position,
            delay_seconds: m.delay_seconds,
            target,
            actions,
        }
    }
}
