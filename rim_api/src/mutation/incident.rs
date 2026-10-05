use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct IncidentMutations;

#[Object]
impl IncidentMutations {
    // ── Incidents ──────────────────────────────────────────────────────

    async fn start_incident(
        &self,
        ctx: &Context<'_>,
        input: StartIncidentInput,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let escalation_policy_id = input
            .escalation_policy_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>)
            .transpose()?;

        let incident = svc
            .start_incident(rim_services::incident::command::StartIncidentCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                title: input.title,
                severity: input.severity.map(Into::into),
                // An explicit override wins; `None` falls back to the conversation's default
                // policy (§3/§10).
                escalation_policy_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn update_incident_status(
        &self,
        ctx: &Context<'_>,
        input: UpdateIncidentStatusInput,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;

        let incident = svc
            .update_incident_status(rim_services::incident::command::UpdateIncidentStatusCmd {
                actor: Actor::new(actor),
                incident_id,
                status: input.status.into(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn advance_incident_phase(
        &self,
        ctx: &Context<'_>,
        input: AdvanceIncidentPhaseInput,
    ) -> Result<crate::types::IncidentPhaseGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;

        let phase = svc
            .advance_phase(rim_services::incident::command::AdvancePhaseCmd {
                actor: Actor::new(actor),
                incident_id,
                next_phase: input.next_phase.into(),
                note: input.note,
            })
            .await
            .map_err(map_service_error)?;

        Ok(phase.into())
    }

    async fn tag_incident_message(
        &self,
        ctx: &Context<'_>,
        input: TagIncidentMessageInput,
    ) -> Result<crate::types::IncidentMessageTagGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id =
            parse_uuid_id::<rim_core::incident::model::IncidentId>(&input.incident_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let tag = svc
            .tag_message(rim_services::incident::command::TagMessageCmd {
                actor: Actor::new(actor),
                incident_id,
                message_id,
                tag_type: input.tag_type.map(Into::into),
            })
            .await
            .map_err(map_service_error)?;

        Ok(tag.into())
    }

    async fn generate_postmortem(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .generate_postmortem(rim_services::incident::command::GeneratePostmortemCmd {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    async fn resolve_incident(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .resolve_incident(rim_services::incident::command::ResolveIncidentCmd {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }

    /// Acknowledge an incident (§4). Any server member may ack, not owner-gated — whoever
    /// responds picked it up. Idempotent: acking an already-acked incident returns it unchanged.
    /// Cancels the incident's active escalation run, if any.
    async fn acknowledge_incident(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<crate::types::IncidentGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let incident = svc
            .acknowledge_incident(rim_services::incident::command::AcknowledgeIncidentCmd {
                actor: Actor::new(actor),
                incident_id,
                initiator: None,
            })
            .await
            .map_err(map_service_error)?;

        Ok(incident.into())
    }
}

#[derive(InputObject)]
struct StartIncidentInput {
    server_id: ID,
    conversation_id: ID,
    title: String,
    severity: Option<crate::types::IncidentSeverityGql>,
    /// Overrides the conversation's default escalation policy (`setChannelEscalationPolicy`) for
    /// just this incident (§3/§10). `None` falls back to that default (or no escalation chain at
    /// all if none is set).
    escalation_policy_id: Option<ID>,
}

#[derive(InputObject)]
struct UpdateIncidentStatusInput {
    incident_id: ID,
    status: crate::types::IncidentStatusGql,
}

#[derive(InputObject)]
struct AdvanceIncidentPhaseInput {
    incident_id: ID,
    next_phase: crate::types::IncidentPhaseNameGql,
    note: Option<String>,
}

#[derive(InputObject)]
struct TagIncidentMessageInput {
    incident_id: ID,
    message_id: ID,
    tag_type: Option<crate::types::MessageTagTypeGql>,
}
