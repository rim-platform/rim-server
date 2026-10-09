use chrono::{DateTime, Utc};

use rim_core::oncall::model::{
    OnCallOverride, OnCallOverrideId, OnCallSchedule, OnCallScheduleId, RotationKind,
    RotationLayer, RotationLayerId, RotationParticipant, RotationParticipantId,
};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct OnCallScheduleDto {
    pub id: OnCallScheduleId,
    pub server_id: ServerId,
    pub name: String,
    pub timezone: String,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<OnCallSchedule> for OnCallScheduleDto {
    fn from(s: OnCallSchedule) -> Self {
        Self {
            id: s.id,
            server_id: s.server_id,
            name: s.name,
            timezone: s.timezone,
            created_by: s.created_by,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RotationLayerDto {
    pub id: RotationLayerId,
    pub schedule_id: OnCallScheduleId,
    pub name: String,
    pub priority: i32,
    pub kind: RotationKind,
    pub anchor_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// Participants ordered by `position`. Populated by the service (a `JOIN` the repository
    /// layer doesn't do on its own).
    pub participants: Vec<UserId>,
}

#[derive(Debug, Clone)]
pub struct RotationParticipantDto {
    pub id: RotationParticipantId,
    pub layer_id: RotationLayerId,
    pub user_id: UserId,
    pub position: i32,
}

impl From<RotationParticipant> for RotationParticipantDto {
    fn from(p: RotationParticipant) -> Self {
        Self {
            id: p.id,
            layer_id: p.layer_id,
            user_id: p.user_id,
            position: p.position,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OnCallOverrideDto {
    pub id: OnCallOverrideId,
    pub schedule_id: OnCallScheduleId,
    pub user_id: UserId,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub created_by: UserId,
}

impl From<OnCallOverride> for OnCallOverrideDto {
    fn from(o: OnCallOverride) -> Self {
        Self {
            id: o.id,
            schedule_id: o.schedule_id,
            user_id: o.user_id,
            start_at: o.start_at,
            end_at: o.end_at,
            created_by: o.created_by,
        }
    }
}

/// Answer to "who is on call right now" for a schedule (`whoIsOnCall` GraphQL query).
#[derive(Debug, Clone)]
pub struct OnCallNowDto {
    pub schedule_id: OnCallScheduleId,
    /// `None` if no override applies and every layer is empty (nobody on call).
    pub current_user_id: Option<UserId>,
    /// When the current assignment ends: the active override's `end_at` if one applies, else the
    /// next rotation handoff boundary of the layer currently on call. `None` if there's no
    /// currently-defined end (e.g. `current_user_id` is `None`, or a `Custom` layer with a
    /// degenerate `interval_seconds`).
    pub shift_ends_at: Option<DateTime<Utc>>,
    /// Who is on call immediately after `shift_ends_at`. `None` under the same conditions as
    /// `shift_ends_at`, or if the answer can't be determined (e.g. no participants at all).
    pub next_user_id: Option<UserId>,
}

/// One contiguous block of on-call coverage (`onCallShifts` GraphQL query) — a 1:1 mapping of
/// [`rim_core::oncall::resolve::OnCallShift`].
#[derive(Debug, Clone, Copy)]
pub struct OnCallShiftDto {
    /// `None` if nobody was covering during this block (an empty schedule/layer).
    pub user_id: Option<UserId>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    /// `true` if this block came from an `OnCallOverride` rather than the base rotation layer
    /// (§10 FLAG B).
    pub is_override: bool,
}

impl From<rim_core::oncall::resolve::OnCallShift> for OnCallShiftDto {
    fn from(s: rim_core::oncall::resolve::OnCallShift) -> Self {
        Self {
            user_id: s.user_id,
            start_at: s.start_at,
            end_at: s.end_at,
            is_override: s.is_override,
        }
    }
}

/// Builds the layer-plus-participants shape [`crate::oncall::service::OnCallService`]
/// implementations resolve from `RotationLayer` + `RotationParticipant` rows before feeding
/// them into [`rim_core::oncall::resolve::resolve_on_call`].
pub fn rotation_layer_dto(layer: RotationLayer, participants: Vec<UserId>) -> RotationLayerDto {
    RotationLayerDto {
        id: layer.id,
        schedule_id: layer.schedule_id,
        name: layer.name,
        priority: layer.priority,
        kind: layer.kind,
        anchor_at: layer.anchor_at,
        created_at: layer.created_at,
        participants,
    }
}
