use async_graphql::{Enum, Error, InputObject, Result, SimpleObject};

// ── Escalation policies (Track C1b/C1c) ─────────────────────────────────────

/// Discriminator for [`EscalationStepGql::target_kind`] / [`EscalationTargetInputGql::kind`].
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum EscalationTargetKindGql {
    User,
    OnCall,
}

/// An `EscalationTarget`, flattened for GraphQL: `kind` selects which of `userId`/`scheduleId`
/// is populated (GraphQL has no native tagged-union input type) — same flattened-union pattern
/// as [`RotationKindInputGql`], including strict rejection of a field that belongs to the
/// *other* kind.
#[derive(InputObject, Clone, Debug)]
pub struct EscalationTargetInputGql {
    pub kind: EscalationTargetKindGql,
    /// Required for `USER`; must not be set for `ON_CALL`.
    pub user_id: Option<async_graphql::ID>,
    /// Required for `ON_CALL`; must not be set for `USER`.
    pub schedule_id: Option<async_graphql::ID>,
}

impl TryFrom<EscalationTargetInputGql> for rim_core::escalation::model::EscalationTarget {
    type Error = Error;

    fn try_from(value: EscalationTargetInputGql) -> Result<Self> {
        use rim_core::escalation::model::EscalationTarget;

        match value.kind {
            EscalationTargetKindGql::User => {
                if value.schedule_id.is_some() {
                    return Err(Error::new(
                        "scheduleId must not be set for USER".to_string(),
                    ));
                }
                let user_id = value
                    .user_id
                    .ok_or_else(|| Error::new("userId is required for USER"))?;
                Ok(EscalationTarget::User(crate::ids::parse_uuid_id::<
                    rim_core::user::model::UserId,
                >(&user_id)?))
            }
            EscalationTargetKindGql::OnCall => {
                if value.user_id.is_some() {
                    return Err(Error::new("userId must not be set for ON_CALL".to_string()));
                }
                let schedule_id = value
                    .schedule_id
                    .ok_or_else(|| Error::new("scheduleId is required for ON_CALL"))?;
                Ok(EscalationTarget::OnCall(crate::ids::parse_uuid_id::<
                    rim_core::oncall::model::OnCallScheduleId,
                >(&schedule_id)?))
            }
        }
    }
}

/// `MENTION_IN_CHANNEL`/`EMAIL` — `PUSH` is reserved for a future track and intentionally has no
/// GraphQL representation yet (mirrors `rim_core::escalation::model::EscalationAction`, which has
/// no `Push` variant either — see that enum's doc comment).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum EscalationActionKindGql {
    MentionInChannel,
    Email,
}

impl From<rim_core::escalation::model::EscalationAction> for EscalationActionKindGql {
    fn from(value: rim_core::escalation::model::EscalationAction) -> Self {
        use rim_core::escalation::model::EscalationAction;
        match value {
            EscalationAction::MentionInChannel => Self::MentionInChannel,
            EscalationAction::Email => Self::Email,
        }
    }
}

impl From<EscalationActionKindGql> for rim_core::escalation::model::EscalationAction {
    fn from(value: EscalationActionKindGql) -> Self {
        match value {
            EscalationActionKindGql::MentionInChannel => Self::MentionInChannel,
            EscalationActionKindGql::Email => Self::Email,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum EscalationRunStatusGql {
    Active,
    Cancelled,
    Completed,
}

impl From<rim_core::escalation::model::EscalationRunStatus> for EscalationRunStatusGql {
    fn from(value: rim_core::escalation::model::EscalationRunStatus) -> Self {
        use rim_core::escalation::model::EscalationRunStatus as D;
        match value {
            D::Active => Self::Active,
            D::Cancelled => Self::Cancelled,
            D::Completed => Self::Completed,
        }
    }
}

/// A step input for `createEscalationPolicy`/`updateEscalationPolicy` — no `id`/`policyId`
/// (assigned by the repository) since the full step list is replaced atomically (§3).
#[derive(InputObject, Clone, Debug)]
pub struct EscalationStepInputGql {
    pub delay_seconds: i64,
    pub target: EscalationTargetInputGql,
    /// At least one action is required — enforced at the service boundary.
    pub actions: Vec<EscalationActionKindGql>,
}

impl TryFrom<EscalationStepInputGql> for rim_services::escalation::dto::EscalationStepInput {
    type Error = Error;

    fn try_from(value: EscalationStepInputGql) -> Result<Self> {
        Ok(Self {
            delay_seconds: value.delay_seconds,
            target: value.target.try_into()?,
            actions: value.actions.into_iter().map(Into::into).collect(),
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct EscalationStepGql {
    pub id: async_graphql::ID,
    pub policy_id: async_graphql::ID,
    /// 0-based order within the policy.
    pub position: i32,
    pub delay_seconds: i64,
    pub target_kind: EscalationTargetKindGql,
    /// Present only when `targetKind` is `USER`.
    pub target_user_id: Option<async_graphql::ID>,
    /// Present only when `targetKind` is `ON_CALL`.
    pub target_schedule_id: Option<async_graphql::ID>,
    pub actions: Vec<EscalationActionKindGql>,
}

impl From<rim_services::escalation::dto::EscalationStepDto> for EscalationStepGql {
    fn from(value: rim_services::escalation::dto::EscalationStepDto) -> Self {
        use rim_core::escalation::model::EscalationTarget;

        let (target_kind, target_user_id, target_schedule_id) = match value.target {
            EscalationTarget::User(user_id) => (
                EscalationTargetKindGql::User,
                Some(user_id.to_string().into()),
                None,
            ),
            EscalationTarget::OnCall(schedule_id) => (
                EscalationTargetKindGql::OnCall,
                None,
                Some(schedule_id.to_string().into()),
            ),
        };

        Self {
            id: value.id.to_string().into(),
            policy_id: value.policy_id.to_string().into(),
            position: value.position,
            delay_seconds: value.delay_seconds,
            target_kind,
            target_user_id,
            target_schedule_id,
            actions: value.actions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct EscalationPolicyGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    pub created_by: async_graphql::ID,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Ordered by `position`.
    pub steps: Vec<EscalationStepGql>,
}

impl From<rim_services::escalation::dto::EscalationPolicyDto> for EscalationPolicyGql {
    fn from(value: rim_services::escalation::dto::EscalationPolicyDto) -> Self {
        Self {
            id: value.id.to_string().into(),
            server_id: value.server_id.to_string().into(),
            name: value.name,
            created_by: value.created_by.to_string().into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            steps: value.steps.into_iter().map(Into::into).collect(),
        }
    }
}

/// Read projection of the DB `escalation_run` row for a live incident (§10 FLAG C) — backs
/// `Incident.escalationRun`. See `IncidentGql`'s `escalation_run` resolver for the
/// nullability rationale.
#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentEscalationRunGql {
    pub current_step_index: i32,
    pub next_fire_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: EscalationRunStatusGql,
}

impl From<rim_services::escalation::dto::EscalationRunDto> for IncidentEscalationRunGql {
    fn from(value: rim_services::escalation::dto::EscalationRunDto) -> Self {
        Self {
            current_step_index: value.current_step_index,
            next_fire_at: value.next_fire_at,
            status: value.status.into(),
        }
    }
}

#[cfg(test)]
mod escalation_target_input_tests {
    use super::{EscalationTargetInputGql, EscalationTargetKindGql};
    use rim_core::escalation::model::EscalationTarget;
    use rim_core::oncall::model::OnCallScheduleId;
    use rim_core::user::model::UserId;

    #[test]
    fn user_kind_converts_with_user_id() {
        let user_id = UserId::new();
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::User,
            user_id: Some(user_id.to_string().into()),
            schedule_id: None,
        };
        let target: EscalationTarget = input.try_into().unwrap();
        assert_eq!(target, EscalationTarget::User(user_id));
    }

    #[test]
    fn on_call_kind_converts_with_schedule_id() {
        let schedule_id = OnCallScheduleId::new();
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::OnCall,
            user_id: None,
            schedule_id: Some(schedule_id.to_string().into()),
        };
        let target: EscalationTarget = input.try_into().unwrap();
        assert_eq!(target, EscalationTarget::OnCall(schedule_id));
    }

    #[test]
    fn user_kind_rejects_schedule_id_set() {
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::User,
            user_id: Some(UserId::new().to_string().into()),
            schedule_id: Some(OnCallScheduleId::new().to_string().into()),
        };
        let err: Result<EscalationTarget, _> = input.try_into();
        assert!(err.is_err());
    }

    #[test]
    fn on_call_kind_rejects_user_id_set() {
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::OnCall,
            user_id: Some(UserId::new().to_string().into()),
            schedule_id: Some(OnCallScheduleId::new().to_string().into()),
        };
        let err: Result<EscalationTarget, _> = input.try_into();
        assert!(err.is_err());
    }

    #[test]
    fn user_kind_requires_user_id() {
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::User,
            user_id: None,
            schedule_id: None,
        };
        let err: Result<EscalationTarget, _> = input.try_into();
        assert!(err.is_err());
    }

    #[test]
    fn on_call_kind_requires_schedule_id() {
        let input = EscalationTargetInputGql {
            kind: EscalationTargetKindGql::OnCall,
            user_id: None,
            schedule_id: None,
        };
        let err: Result<EscalationTarget, _> = input.try_into();
        assert!(err.is_err());
    }
}
