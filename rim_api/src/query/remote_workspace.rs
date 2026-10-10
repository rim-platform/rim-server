use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::federation::relay;
use crate::types::{RemoteSubscription, RevocationFanout};

#[derive(Default)]
pub(crate) struct RemoteWorkspaceQueries;

#[Object]
impl RemoteWorkspaceQueries {
    /// The caller's subscriptions to workspaces on other hosts.
    async fn remote_subscriptions(&self, ctx: &Context<'_>) -> Result<Vec<RemoteSubscription>> {
        let actor = require_actor(ctx)?;
        Ok(relay(ctx)?
            .workspace_relay
            .remote_subscriptions(Actor::new(actor))
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Where the revocation of one of the caller's devices stands at each host holding them.
    async fn device_revocation_status(
        &self,
        ctx: &Context<'_>,
        device_id: ID,
    ) -> Result<Vec<RevocationFanout>> {
        let actor = require_actor(ctx)?;
        let device_id = uuid::Uuid::parse_str(&device_id)
            .map_err(|_| Error::new("deviceId is not a device id"))?;
        Ok(relay(ctx)?
            .workspace_relay
            .device_revocation_status(Actor::new(actor), device_id)
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}
