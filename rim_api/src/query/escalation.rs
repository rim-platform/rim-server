use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;

#[derive(Default)]
pub(crate) struct EscalationQueries;

#[Object]
impl EscalationQueries {
    /// List a server's escalation policies. Any workspace member may read.
    async fn escalation_policies(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<crate::types::EscalationPolicyGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::escalation::service::EscalationPolicyService>>(
            )?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let policies = svc
            .list_policies(
                rim_services::escalation::command::ListEscalationPoliciesQuery {
                    actor: Actor::new(actor),
                    server_id,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(policies.into_iter().map(Into::into).collect())
    }
}
