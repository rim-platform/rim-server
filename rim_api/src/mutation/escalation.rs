use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct EscalationMutations;

#[Object]
impl EscalationMutations {
    // ── Escalation policies (Track C1b/C1c) ───────────────────────────

    /// Create an escalation policy. Owner+Moderator (server-scoped — per-channel moderator
    /// roles don't apply). Every step target is validated to belong to this server (C1 security
    /// review): a `User` target must be a current member, an `OnCall` target's schedule must
    /// belong to this server.
    async fn create_escalation_policy(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
        steps: Vec<crate::types::EscalationStepInputGql>,
    ) -> Result<crate::types::EscalationPolicyGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let steps = steps
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?;

        let policy = svc
            .create_policy(
                rim_services::escalation::command::CreateEscalationPolicyCmd {
                    actor: Actor::new(actor),
                    server_id,
                    name,
                    steps,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(policy.into())
    }

    /// Update an escalation policy's `name` and/or full step list. `steps: Some(_)` replaces
    /// every step atomically; `None` leaves the existing steps untouched. Owner+Moderator
    /// (resolved via the policy's own server); same per-target server constraint as create.
    async fn update_escalation_policy(
        &self,
        ctx: &Context<'_>,
        policy_id: ID,
        name: Option<String>,
        steps: Option<Vec<crate::types::EscalationStepInputGql>>,
    ) -> Result<crate::types::EscalationPolicyGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;
        let steps = steps
            .map(|steps| {
                steps
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;

        let policy = svc
            .update_policy(
                rim_services::escalation::command::UpdateEscalationPolicyCmd {
                    actor: Actor::new(actor),
                    policy_id,
                    name,
                    steps,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(policy.into())
    }

    /// Delete an escalation policy. Owner+Moderator (resolved via the policy's own server).
    async fn delete_escalation_policy(&self, ctx: &Context<'_>, policy_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;

        svc.delete_policy(
            rim_services::escalation::command::DeleteEscalationPolicyCmd {
                actor: Actor::new(actor),
                policy_id,
            },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Set a conversation's default escalation policy — the policy `ingestAlert`-declared
    /// incidents always use (there's no human to pick an override for an alert-declared
    /// incident), and `startIncident`'s fallback when no per-call override is given.
    /// Owner+Moderator.
    async fn set_channel_escalation_policy(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        policy_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let policy_id =
            parse_uuid_id::<rim_core::escalation::model::EscalationPolicyId>(&policy_id)?;

        svc.set_channel_policy(
            rim_services::escalation::command::SetChannelEscalationPolicyCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                policy_id,
            },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}
