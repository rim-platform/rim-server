use chrono::{DateTime, Utc};

use rim_core::oncall::model::{OnCallOverrideId, OnCallScheduleId, RotationKind, RotationLayerId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::Actor;

#[derive(Debug, Clone)]
pub struct CreateOnCallScheduleCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    /// IANA timezone name, validated against `chrono_tz::Tz` by the service.
    pub timezone: String,
}

#[derive(Debug, Clone)]
pub struct UpdateOnCallScheduleCmd {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
    pub name: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteOnCallScheduleCmd {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
}

#[derive(Debug, Clone)]
pub struct ListOnCallSchedulesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// `OnCallSchedule.rotationLayers` (§10) — lists a schedule's existing layers (each with its
/// participants) so the frontend editor can load and edit them, not just create new ones.
#[derive(Debug, Clone)]
pub struct ListRotationLayersQuery {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
}

#[derive(Debug, Clone)]
pub struct AddRotationLayerCmd {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
    pub name: String,
    pub priority: i32,
    pub kind: RotationKind,
    pub anchor_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateRotationLayerCmd {
    pub actor: Actor,
    pub layer_id: RotationLayerId,
    pub name: Option<String>,
    pub priority: Option<i32>,
    pub kind: Option<RotationKind>,
    pub anchor_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct RemoveRotationLayerCmd {
    pub actor: Actor,
    pub layer_id: RotationLayerId,
}

/// Replace a layer's full ordered participant list. `user_ids[0]` becomes the first shift.
#[derive(Debug, Clone)]
pub struct SetRotationParticipantsCmd {
    pub actor: Actor,
    pub layer_id: RotationLayerId,
    pub user_ids: Vec<UserId>,
}

#[derive(Debug, Clone)]
pub struct CreateOnCallOverrideCmd {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
    pub user_id: UserId,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DeleteOnCallOverrideCmd {
    pub actor: Actor,
    pub override_id: OnCallOverrideId,
}

/// `whoIsOnCall(scheduleId)` — `at` defaults to "now" when `None` (mainly useful for tests /
/// previewing a schedule's future coverage).
#[derive(Debug, Clone)]
pub struct WhoIsOnCallQuery {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
    pub at: Option<DateTime<Utc>>,
}

/// `serverOnCall(serverId)` — batch "who is on call now" across every schedule on a server, so
/// a status-bar-style UI doesn't need one `whoIsOnCall` round-trip per schedule.
#[derive(Debug, Clone)]
pub struct ServerOnCallQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// `onCallShifts(scheduleId, from, to)` — the resolved coverage timeline over `[from, to)`. The
/// service rejects a span over 90 days (`ServiceError::Validation`) to keep the boundary walk
/// bounded — see `rim_core::oncall::resolve::resolve_shifts`'s doc comment for why the cap lives
/// here rather than in the pure function.
#[derive(Debug, Clone)]
pub struct ListOnCallShiftsQuery {
    pub actor: Actor,
    pub schedule_id: OnCallScheduleId,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}
