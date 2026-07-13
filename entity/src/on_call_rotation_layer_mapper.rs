use chrono::NaiveTime;

use crate::on_call_rotation_layer;
use rim_core::oncall::model::{RotationKind, RotationKindTag, weekday_from_i16};

/// Fallback used when a stored per-kind field is unexpectedly missing/invalid (should never
/// happen for rows written by this service — the write path always populates the fields that
/// match `kind` — but a pure `From` impl must not panic on unexpected DB state).
const FALLBACK_HANDOFF_TIME: NaiveTime = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
const FALLBACK_INTERVAL_SECONDS: i64 = 3600;

impl From<on_call_rotation_layer::Model> for rim_core::oncall::model::RotationLayer {
    fn from(m: on_call_rotation_layer::Model) -> Self {
        let layer_id = m.id;
        let tag = m.kind.parse().unwrap_or_else(|_| {
            tracing::warn!(layer_id = %layer_id, kind = %m.kind, "unknown rotation kind, defaulting to Custom");
            RotationKindTag::Custom
        });

        let kind = match tag {
            RotationKindTag::Daily => RotationKind::Daily {
                handoff_time: m.handoff_time.unwrap_or_else(|| {
                    tracing::warn!(layer_id = %layer_id, "daily rotation layer missing handoff_time");
                    FALLBACK_HANDOFF_TIME
                }),
            },
            RotationKindTag::Weekly => RotationKind::Weekly {
                handoff_weekday: m
                    .handoff_weekday
                    .and_then(weekday_from_i16)
                    .unwrap_or_else(|| {
                        tracing::warn!(layer_id = %layer_id, "weekly rotation layer missing/invalid handoff_weekday");
                        chrono::Weekday::Mon
                    }),
                handoff_time: m.handoff_time.unwrap_or_else(|| {
                    tracing::warn!(layer_id = %layer_id, "weekly rotation layer missing handoff_time");
                    FALLBACK_HANDOFF_TIME
                }),
            },
            RotationKindTag::Custom => RotationKind::Custom {
                interval_seconds: m.interval_seconds.unwrap_or_else(|| {
                    tracing::warn!(layer_id = %layer_id, "custom rotation layer missing interval_seconds");
                    FALLBACK_INTERVAL_SECONDS
                }),
            },
        };

        Self {
            id: m.id.into(),
            schedule_id: m.schedule_id.into(),
            name: m.name,
            priority: m.priority,
            kind,
            anchor_at: m.anchor_at.into(),
            created_at: m.created_at.into(),
        }
    }
}
