use async_graphql::{ComplexObject, Context, Enum, Error, InputObject, Result, SimpleObject};

use super::User;
use crate::resolve::ResolveExt;

// ── On-call schedules (Track C1a) ───────────────────────────────────────────

/// Discriminator for [`RotationLayerGql::kind`] / [`RotationKindInputGql::kind`]. See
/// `docs/oncall.md` for the timezone/DST semantics of each variant.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RotationKindGql {
    Daily,
    Weekly,
    Custom,
}

/// A `RotationKind`, flattened for GraphQL: `kind` selects which of the other fields are
/// populated (GraphQL has no native tagged-union input type). `handoffTime` is `"HH:MM:SS"`
/// 24-hour wall-clock local time.
#[derive(InputObject, Clone, Debug)]
pub struct RotationKindInputGql {
    pub kind: RotationKindGql,
    /// Required for `DAILY`/`WEEKLY`; ignored otherwise.
    pub handoff_time: Option<String>,
    /// `0` (Monday) .. `6` (Sunday). Required for `WEEKLY`; ignored otherwise.
    pub handoff_weekday: Option<i32>,
    /// Required for `CUSTOM` (must be `> 0`); ignored otherwise.
    pub interval_seconds: Option<i64>,
}

impl TryFrom<RotationKindInputGql> for rim_core::oncall::model::RotationKind {
    type Error = Error;

    fn try_from(value: RotationKindInputGql) -> Result<Self> {
        use rim_core::oncall::model::RotationKind;

        let parse_time = |raw: &str| -> Result<chrono::NaiveTime> {
            chrono::NaiveTime::parse_from_str(raw, "%H:%M:%S")
                .or_else(|_| chrono::NaiveTime::parse_from_str(raw, "%H:%M"))
                .map_err(|_| Error::new(format!("invalid handoffTime {raw:?}, expected HH:MM:SS")))
        };

        // Strict flattened-union rejection: a field that belongs to a *different* kind than the
        // one selected is almost always a client bug (e.g. sent `intervalSeconds` alongside
        // `kind: DAILY`) — reject loudly rather than silently discarding it.
        match value.kind {
            RotationKindGql::Daily | RotationKindGql::Weekly => {
                if value.interval_seconds.is_some() {
                    return Err(Error::new(format!(
                        "intervalSeconds must not be set for {:?}",
                        value.kind
                    )));
                }
            }
            RotationKindGql::Custom => {
                if value.handoff_time.is_some() || value.handoff_weekday.is_some() {
                    return Err(Error::new(
                        "handoffTime/handoffWeekday must not be set for CUSTOM".to_string(),
                    ));
                }
            }
        }
        if value.kind == RotationKindGql::Daily && value.handoff_weekday.is_some() {
            return Err(Error::new(
                "handoffWeekday must not be set for DAILY".to_string(),
            ));
        }

        match value.kind {
            RotationKindGql::Daily => {
                let raw = value
                    .handoff_time
                    .ok_or_else(|| Error::new("handoffTime is required for DAILY"))?;
                Ok(RotationKind::Daily {
                    handoff_time: parse_time(&raw)?,
                })
            }
            RotationKindGql::Weekly => {
                let raw_time = value
                    .handoff_time
                    .ok_or_else(|| Error::new("handoffTime is required for WEEKLY"))?;
                let raw_weekday = value
                    .handoff_weekday
                    .ok_or_else(|| Error::new("handoffWeekday is required for WEEKLY"))?;
                let handoff_weekday = rim_core::oncall::model::weekday_from_i16(raw_weekday as i16)
                    .ok_or_else(|| {
                        Error::new("handoffWeekday must be 0 (Monday) through 6 (Sunday)")
                    })?;
                Ok(RotationKind::Weekly {
                    handoff_weekday,
                    handoff_time: parse_time(&raw_time)?,
                })
            }
            RotationKindGql::Custom => {
                let interval_seconds = value
                    .interval_seconds
                    .ok_or_else(|| Error::new("intervalSeconds is required for CUSTOM"))?;
                Ok(RotationKind::Custom { interval_seconds })
            }
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct OnCallScheduleGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    pub timezone: String,
    pub created_by: async_graphql::ID,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::oncall::dto::OnCallScheduleDto> for OnCallScheduleGql {
    fn from(value: rim_services::oncall::dto::OnCallScheduleDto) -> Self {
        Self {
            id: value.id.to_string().into(),
            server_id: value.server_id.to_string().into(),
            name: value.name,
            timezone: value.timezone,
            created_by: value.created_by.to_string().into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[ComplexObject]
impl OnCallScheduleGql {
    /// This schedule's existing rotation layers, each with its participants attached, ordered
    /// `priority DESC, created_at ASC` (§10) — lets the frontend editor load and edit existing
    /// rotations rather than only being able to create new ones. Any server member may read.
    async fn rotation_layers(&self, ctx: &Context<'_>) -> Result<Vec<RotationLayerGql>> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id =
            crate::ids::parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&self.id)?;

        let layers = svc
            .list_rotation_layers(rim_services::oncall::command::ListRotationLayersQuery {
                actor: rim_services::Actor::new(actor),
                schedule_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(layers.into_iter().map(Into::into).collect())
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct RotationLayerGql {
    pub id: async_graphql::ID,
    pub schedule_id: async_graphql::ID,
    pub name: String,
    pub priority: i32,
    pub kind: RotationKindGql,
    /// `"HH:MM:SS"`, present only when `kind` is `DAILY`/`WEEKLY`.
    pub handoff_time: Option<String>,
    /// `0` (Monday) .. `6` (Sunday), present only when `kind` is `WEEKLY`.
    pub handoff_weekday: Option<i32>,
    /// Present only when `kind` is `CUSTOM`.
    pub interval_seconds: Option<i64>,
    pub anchor_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Ordered by shift position.
    pub participants: Vec<async_graphql::ID>,
}

impl From<rim_services::oncall::dto::RotationLayerDto> for RotationLayerGql {
    fn from(value: rim_services::oncall::dto::RotationLayerDto) -> Self {
        use rim_core::oncall::model::RotationKind;

        let (kind, handoff_time, handoff_weekday, interval_seconds) = match value.kind {
            RotationKind::Daily { handoff_time } => (
                RotationKindGql::Daily,
                Some(handoff_time.format("%H:%M:%S").to_string()),
                None,
                None,
            ),
            RotationKind::Weekly {
                handoff_weekday,
                handoff_time,
            } => (
                RotationKindGql::Weekly,
                Some(handoff_time.format("%H:%M:%S").to_string()),
                Some(rim_core::oncall::model::weekday_to_i16(handoff_weekday) as i32),
                None,
            ),
            RotationKind::Custom { interval_seconds } => {
                (RotationKindGql::Custom, None, None, Some(interval_seconds))
            }
        };

        Self {
            id: value.id.to_string().into(),
            schedule_id: value.schedule_id.to_string().into(),
            name: value.name,
            priority: value.priority,
            kind,
            handoff_time,
            handoff_weekday,
            interval_seconds,
            anchor_at: value.anchor_at,
            created_at: value.created_at,
            participants: value
                .participants
                .into_iter()
                .map(|id| id.to_string().into())
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct OnCallOverrideGql {
    pub id: async_graphql::ID,
    pub schedule_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub start_at: chrono::DateTime<chrono::Utc>,
    pub end_at: chrono::DateTime<chrono::Utc>,
    pub created_by: async_graphql::ID,
}

impl From<rim_services::oncall::dto::OnCallOverrideDto> for OnCallOverrideGql {
    fn from(value: rim_services::oncall::dto::OnCallOverrideDto) -> Self {
        Self {
            id: value.id.to_string().into(),
            schedule_id: value.schedule_id.to_string().into(),
            user_id: value.user_id.to_string().into(),
            start_at: value.start_at,
            end_at: value.end_at,
            created_by: value.created_by.to_string().into(),
        }
    }
}

/// Answer to "who is on call now" (`whoIsOnCall`/`serverOnCall` queries). `currentUser`/`nextUser`
/// are resolved via the shared `User` DataLoader (the `message.author` pattern) rather than
/// exposed as bare ids.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct OnCallNowGql {
    pub schedule_id: async_graphql::ID,
    #[graphql(skip)]
    pub current_user_id: Option<async_graphql::ID>,
    pub shift_ends_at: Option<chrono::DateTime<chrono::Utc>>,
    #[graphql(skip)]
    pub next_user_id: Option<async_graphql::ID>,
}

impl From<rim_services::oncall::dto::OnCallNowDto> for OnCallNowGql {
    fn from(value: rim_services::oncall::dto::OnCallNowDto) -> Self {
        Self {
            schedule_id: value.schedule_id.to_string().into(),
            current_user_id: value.current_user_id.map(|id| id.to_string().into()),
            shift_ends_at: value.shift_ends_at,
            next_user_id: value.next_user_id.map(|id| id.to_string().into()),
        }
    }
}

#[ComplexObject]
impl OnCallNowGql {
    async fn current_user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match &self.current_user_id {
            Some(id) => ctx.load_user(id).await,
            None => Ok(None),
        }
    }

    async fn next_user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match &self.next_user_id {
            Some(id) => ctx.load_user(id).await,
            None => Ok(None),
        }
    }
}

/// One contiguous block of on-call coverage (`onCallShifts` query). `user` is resolved via the
/// shared `User` DataLoader, not exposed as a bare id.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct OnCallShiftGql {
    #[graphql(skip)]
    pub user_id: Option<async_graphql::ID>,
    pub start_at: chrono::DateTime<chrono::Utc>,
    pub end_at: chrono::DateTime<chrono::Utc>,
    /// `true` if this block came from a one-off `OnCallOverride` rather than the base rotation
    /// layer (§10 FLAG B) — lets the frontend render override segments distinctly.
    pub is_override: bool,
}

impl From<rim_services::oncall::dto::OnCallShiftDto> for OnCallShiftGql {
    fn from(value: rim_services::oncall::dto::OnCallShiftDto) -> Self {
        Self {
            user_id: value.user_id.map(|id| id.to_string().into()),
            start_at: value.start_at,
            end_at: value.end_at,
            is_override: value.is_override,
        }
    }
}

#[ComplexObject]
impl OnCallShiftGql {
    async fn user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match &self.user_id {
            Some(id) => ctx.load_user(id).await,
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod rotation_kind_input_tests {
    use super::{RotationKindGql, RotationKindInputGql};
    use rim_core::oncall::model::RotationKind;

    #[test]
    fn daily_converts_with_only_handoff_time() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Daily,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: None,
            interval_seconds: None,
        };
        let kind = RotationKind::try_from(input).expect("valid DAILY input must convert");
        assert!(matches!(kind, RotationKind::Daily { .. }));
    }

    #[test]
    fn weekly_converts_with_handoff_time_and_weekday() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Weekly,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: Some(0),
            interval_seconds: None,
        };
        let kind = RotationKind::try_from(input).expect("valid WEEKLY input must convert");
        assert!(matches!(kind, RotationKind::Weekly { .. }));
    }

    #[test]
    fn custom_converts_with_only_interval_seconds() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Custom,
            handoff_time: None,
            handoff_weekday: None,
            interval_seconds: Some(3600),
        };
        let kind = RotationKind::try_from(input).expect("valid CUSTOM input must convert");
        assert!(matches!(
            kind,
            RotationKind::Custom {
                interval_seconds: 3600
            }
        ));
    }

    /// Strict flattened-union rejection (C1a-tail nit): a field belonging to a *different* kind
    /// than the one selected is rejected rather than silently discarded.
    #[test]
    fn daily_with_interval_seconds_set_is_rejected() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Daily,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: None,
            interval_seconds: Some(300),
        };
        assert!(RotationKind::try_from(input).is_err());
    }

    #[test]
    fn daily_with_handoff_weekday_set_is_rejected() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Daily,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: Some(2),
            interval_seconds: None,
        };
        assert!(RotationKind::try_from(input).is_err());
    }

    #[test]
    fn weekly_with_interval_seconds_set_is_rejected() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Weekly,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: Some(0),
            interval_seconds: Some(300),
        };
        assert!(RotationKind::try_from(input).is_err());
    }

    #[test]
    fn custom_with_handoff_time_set_is_rejected() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Custom,
            handoff_time: Some("09:00:00".to_string()),
            handoff_weekday: None,
            interval_seconds: Some(3600),
        };
        assert!(RotationKind::try_from(input).is_err());
    }

    #[test]
    fn custom_with_handoff_weekday_set_is_rejected() {
        let input = RotationKindInputGql {
            kind: RotationKindGql::Custom,
            handoff_time: None,
            handoff_weekday: Some(3),
            interval_seconds: Some(3600),
        };
        assert!(RotationKind::try_from(input).is_err());
    }
}
