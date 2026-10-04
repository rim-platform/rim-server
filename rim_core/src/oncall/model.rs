use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, NaiveTime, Utc, Weekday};

use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(OnCallScheduleId);
model_id!(RotationLayerId);
model_id!(RotationParticipantId);
model_id!(OnCallOverrideId);

// ── RotationKind ───────────────────────────────────────────────────────────

/// How a rotation layer hands off between participants.
///
/// **Timezone design decision:** `Daily`/`Weekly` hand off at a *wall-clock* local time (in the
/// owning schedule's timezone — see [`OnCallSchedule::timezone`]) so the handoff doesn't drift an
/// hour across a DST transition. `Custom` is a pure UTC duration with no wall-clock alignment —
/// simplest, and the correct semantics for "every 6 hours" style rotations. See `docs/oncall.md`
/// for the full rationale and the DST edge-case handling in
/// [`crate::oncall::resolve::layer_on_call`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RotationKind {
    /// Hand off every day at `handoff_time` local wall-clock time.
    Daily { handoff_time: NaiveTime },
    /// Hand off every 7 days, on `handoff_weekday` at `handoff_time` local wall-clock time.
    Weekly {
        handoff_weekday: Weekday,
        handoff_time: NaiveTime,
    },
    /// Hand off every `interval_seconds` of elapsed UTC time — no wall-clock alignment, DST-agnostic.
    Custom { interval_seconds: i64 },
}

impl RotationKind {
    /// The DB discriminator string stored in `on_call_rotation_layer.kind`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Daily { .. } => "daily",
            Self::Weekly { .. } => "weekly",
            Self::Custom { .. } => "custom",
        }
    }
}

impl fmt::Display for RotationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Maps a [`Weekday`] to the `smallint` stored in `on_call_rotation_layer.handoff_weekday`
/// (`0` = Monday .. `6` = Sunday, matching [`Weekday::num_days_from_monday`]).
pub fn weekday_to_i16(weekday: Weekday) -> i16 {
    weekday.num_days_from_monday() as i16
}

/// Inverse of [`weekday_to_i16`]. Returns `None` for any value outside `0..=6`.
pub fn weekday_from_i16(value: i16) -> Option<Weekday> {
    u8::try_from(value)
        .ok()
        .and_then(|n| Weekday::try_from(n).ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRotationKindError {
    input: String,
}

impl fmt::Display for ParseRotationKindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown rotation kind: {}", self.input)
    }
}

impl std::error::Error for ParseRotationKindError {}

/// A bare discriminator, used when parsing the DB `kind` column before the caller has decoded
/// the per-kind fields (`handoff_time`/`handoff_weekday`/`interval_seconds`) — see
/// `entity::on_call_rotation_layer_mapper` for the full reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationKindTag {
    Daily,
    Weekly,
    Custom,
}

impl FromStr for RotationKindTag {
    type Err = ParseRotationKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "custom" => Ok(Self::Custom),
            other => Err(ParseRotationKindError {
                input: other.to_string(),
            }),
        }
    }
}

// ── Domain models ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnCallSchedule {
    pub id: OnCallScheduleId,
    pub server_id: ServerId,
    pub name: String,
    /// IANA timezone name (e.g. `"America/Chicago"`), validated on write via
    /// `chrono_tz::Tz::from_str`. Rotation handoff math for this schedule's `Daily`/`Weekly`
    /// layers runs in this timezone.
    pub timezone: String,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewOnCallSchedule {
    pub server_id: ServerId,
    pub name: String,
    pub timezone: String,
    pub created_by: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationLayer {
    pub id: RotationLayerId,
    pub schedule_id: OnCallScheduleId,
    pub name: String,
    /// Higher priority wins where it has coverage (a non-empty participant list).
    pub priority: i32,
    pub kind: RotationKind,
    /// The UTC instant at which `participants[0]`'s first shift starts.
    pub anchor_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewRotationLayer {
    pub schedule_id: OnCallScheduleId,
    pub name: String,
    pub priority: i32,
    pub kind: RotationKind,
    pub anchor_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationParticipant {
    pub id: RotationParticipantId,
    pub layer_id: RotationLayerId,
    pub user_id: UserId,
    /// 0-based order within the layer's rotation. Unique per `(layer_id, position)`.
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnCallOverride {
    pub id: OnCallOverrideId,
    pub schedule_id: OnCallScheduleId,
    pub user_id: UserId,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub created_by: UserId,
}

#[derive(Debug, Clone)]
pub struct NewOnCallOverride {
    pub schedule_id: OnCallScheduleId,
    pub user_id: UserId,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub created_by: UserId,
}
