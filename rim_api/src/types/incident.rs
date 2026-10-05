use async_graphql::{ComplexObject, Context, Enum, InputObject, Result, SimpleObject};
use rim_core::repository::pagination::Page;

use super::{IncidentEscalationRunGql, PageInfo, User};
use crate::resolve::ResolveExt;

// ── Incident Mode ──────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentStatusGql {
    Active,
    Mitigated,
    Resolved,
}

impl From<rim_core::incident::model::IncidentStatus> for IncidentStatusGql {
    fn from(value: rim_core::incident::model::IncidentStatus) -> Self {
        use rim_core::incident::model::IncidentStatus as D;
        match value {
            D::Active => Self::Active,
            D::Mitigated => Self::Mitigated,
            D::Resolved => Self::Resolved,
        }
    }
}

impl From<IncidentStatusGql> for rim_core::incident::model::IncidentStatus {
    fn from(value: IncidentStatusGql) -> Self {
        match value {
            IncidentStatusGql::Active => Self::Active,
            IncidentStatusGql::Mitigated => Self::Mitigated,
            IncidentStatusGql::Resolved => Self::Resolved,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentSeverityGql {
    Sev1,
    Sev2,
    Sev3,
    Sev4,
}

impl From<rim_core::incident::model::IncidentSeverity> for IncidentSeverityGql {
    fn from(value: rim_core::incident::model::IncidentSeverity) -> Self {
        use rim_core::incident::model::IncidentSeverity as D;
        match value {
            D::Sev1 => Self::Sev1,
            D::Sev2 => Self::Sev2,
            D::Sev3 => Self::Sev3,
            D::Sev4 => Self::Sev4,
        }
    }
}

impl From<IncidentSeverityGql> for rim_core::incident::model::IncidentSeverity {
    fn from(value: IncidentSeverityGql) -> Self {
        match value {
            IncidentSeverityGql::Sev1 => Self::Sev1,
            IncidentSeverityGql::Sev2 => Self::Sev2,
            IncidentSeverityGql::Sev3 => Self::Sev3,
            IncidentSeverityGql::Sev4 => Self::Sev4,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentPhaseNameGql {
    Detection,
    Triage,
    Mitigation,
    Resolution,
    Postmortem,
}

impl From<rim_core::incident::model::IncidentPhaseName> for IncidentPhaseNameGql {
    fn from(value: rim_core::incident::model::IncidentPhaseName) -> Self {
        use rim_core::incident::model::IncidentPhaseName as D;
        match value {
            D::Detection => Self::Detection,
            D::Triage => Self::Triage,
            D::Mitigation => Self::Mitigation,
            D::Resolution => Self::Resolution,
            D::Postmortem => Self::Postmortem,
        }
    }
}

impl From<IncidentPhaseNameGql> for rim_core::incident::model::IncidentPhaseName {
    fn from(value: IncidentPhaseNameGql) -> Self {
        match value {
            IncidentPhaseNameGql::Detection => Self::Detection,
            IncidentPhaseNameGql::Triage => Self::Triage,
            IncidentPhaseNameGql::Mitigation => Self::Mitigation,
            IncidentPhaseNameGql::Resolution => Self::Resolution,
            IncidentPhaseNameGql::Postmortem => Self::Postmortem,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MessageTagTypeGql {
    Decision,
    ActionItem,
    RootCause,
    TimelineEvent,
}

impl From<rim_core::incident::model::MessageTagType> for MessageTagTypeGql {
    fn from(value: rim_core::incident::model::MessageTagType) -> Self {
        use rim_core::incident::model::MessageTagType as D;
        match value {
            D::Decision => Self::Decision,
            D::ActionItem => Self::ActionItem,
            D::RootCause => Self::RootCause,
            D::TimelineEvent => Self::TimelineEvent,
        }
    }
}

impl From<MessageTagTypeGql> for rim_core::incident::model::MessageTagType {
    fn from(value: MessageTagTypeGql) -> Self {
        match value {
            MessageTagTypeGql::Decision => Self::Decision,
            MessageTagTypeGql::ActionItem => Self::ActionItem,
            MessageTagTypeGql::RootCause => Self::RootCause,
            MessageTagTypeGql::TimelineEvent => Self::TimelineEvent,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct IncidentGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub started_by: async_graphql::ID,
    pub status: IncidentStatusGql,
    pub severity: Option<IncidentSeverityGql>,
    pub title: String,
    pub original_topic: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub mitigated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub postmortem_draft: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// The escalation policy armed at declare time, if any (§3/§10). `None` if the incident was
    /// declared without an escalation chain.
    pub escalation_policy_id: Option<async_graphql::ID>,
    /// Set by `acknowledgeIncident` (§4). `None` until acked.
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Bare id companion to `acknowledgedByUser` below — kept alongside it (rather than only
    /// exposing the nested object) so a client that only needs the id avoids a DataLoader round
    /// trip (§10).
    pub acknowledged_by: Option<async_graphql::ID>,
}

impl From<rim_services::incident::dto::IncidentDto> for IncidentGql {
    fn from(value: rim_services::incident::dto::IncidentDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            started_by: async_graphql::ID(value.started_by.to_string()),
            status: value.status.into(),
            severity: value.severity.map(Into::into),
            title: value.title,
            original_topic: value.original_topic,
            started_at: value.started_at,
            mitigated_at: value.mitigated_at,
            resolved_at: value.resolved_at,
            postmortem_draft: value.postmortem_draft,
            created_at: value.created_at,
            updated_at: value.updated_at,
            escalation_policy_id: value.escalation_policy_id.map(|id| id.to_string().into()),
            acknowledged_at: value.acknowledged_at,
            acknowledged_by: value.acknowledged_by.map(|id| id.to_string().into()),
        }
    }
}

#[ComplexObject]
impl IncidentGql {
    /// Nested `User` for `acknowledgedBy`, resolved via the shared `User` DataLoader (the
    /// `message.author` pattern) rather than requiring a second round-trip. `None` if the
    /// incident hasn't been acknowledged.
    async fn acknowledged_by_user(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match &self.acknowledged_by {
            Some(id) => ctx.load_user(id).await,
            None => Ok(None),
        }
    }

    /// The incident's active escalation run, if any (§10 FLAG C) — lets a client loading
    /// mid-incident, or reconnecting, render a live "escalating to L2 in 4:32" countdown (the
    /// `EscalationStepFired` event alone can't give `nextFireAt` to a fresh client). `None` if no
    /// policy is attached, or the run has already completed/been cancelled — only the currently
    /// `Active` run is meaningful to show. Pure read projection of the DB `escalation_run` row;
    /// adds no persistence.
    async fn escalation_run(&self, ctx: &Context<'_>) -> Result<Option<IncidentEscalationRunGql>> {
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            crate::ids::parse_uuid_id::<rim_core::incident::model::IncidentId>(&self.id)?;

        let run = svc
            .get_active_escalation_run(incident_id)
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(run.map(Into::into))
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentPhaseGql {
    pub id: async_graphql::ID,
    pub incident_id: async_graphql::ID,
    pub phase: IncidentPhaseNameGql,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub note: Option<String>,
}

impl From<rim_services::incident::dto::IncidentPhaseDto> for IncidentPhaseGql {
    fn from(value: rim_services::incident::dto::IncidentPhaseDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            incident_id: async_graphql::ID(value.incident_id.to_string()),
            phase: value.phase.into(),
            started_at: value.started_at,
            ended_at: value.ended_at,
            note: value.note,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentMessageTagGql {
    pub message_id: async_graphql::ID,
    pub incident_id: async_graphql::ID,
    pub phase: IncidentPhaseNameGql,
    pub tag_type: Option<MessageTagTypeGql>,
}

impl From<rim_services::incident::dto::IncidentMessageTagDto> for IncidentMessageTagGql {
    fn from(value: rim_services::incident::dto::IncidentMessageTagDto) -> Self {
        Self {
            message_id: async_graphql::ID(value.message_id.to_string()),
            incident_id: async_graphql::ID(value.incident_id.to_string()),
            phase: value.phase.into(),
            tag_type: value.tag_type.map(Into::into),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentTimelineGql {
    pub incident: IncidentGql,
    pub phases: Vec<IncidentPhaseGql>,
    pub tagged_messages: Vec<IncidentMessageTagGql>,
}

impl From<rim_services::incident::dto::IncidentTimelineDto> for IncidentTimelineGql {
    fn from(value: rim_services::incident::dto::IncidentTimelineDto) -> Self {
        Self {
            incident: value.incident.into(),
            phases: value.phases.into_iter().map(Into::into).collect(),
            tagged_messages: value.tagged_messages.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct IncidentListCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub incident_id: async_graphql::ID,
}

impl TryFrom<IncidentListCursorInput> for rim_services::incident::dto::IncidentListPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: IncidentListCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            created_at: value.created_at,
            incident_id: crate::ids::parse_uuid_id::<rim_core::incident::model::IncidentId>(
                &value.incident_id,
            )?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentListCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub incident_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentPage {
    pub items: Vec<IncidentGql>,
    pub next: Option<IncidentListCursorOutput>,
    pub prev: Option<IncidentListCursorOutput>,
    pub page_info: PageInfo,
}

impl IncidentPage {
    pub fn from_page(
        page: Page<
            rim_services::incident::dto::IncidentDto,
            rim_services::incident::dto::IncidentListPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| IncidentListCursorOutput {
                created_at: c.created_at,
                incident_id: async_graphql::ID(c.incident_id.to_string()),
            }),
            prev: page.prev.map(|c| IncidentListCursorOutput {
                created_at: c.created_at,
                incident_id: async_graphql::ID(c.incident_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}
