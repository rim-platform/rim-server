use std::sync::Arc;

use async_graphql::*;
use rim_services::Actor;
use rim_services::dm_policy::service::DmPolicyService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::MyDmSettings;

#[derive(Default)]
pub(crate) struct DmPolicyQueries;

#[Object]
impl DmPolicyQueries {
    /// Who may start a DM with the caller: their choice for people outside their workspaces,
    /// for each workspace's members, and what applies once the operator's and the workspaces'
    /// floors are taken in, with where a refusing floor comes from.
    async fn my_dm_settings(&self, ctx: &Context<'_>) -> Result<MyDmSettings> {
        let actor = require_actor(ctx)?;
        Ok(ctx
            .data::<Arc<dyn DmPolicyService>>()?
            .my_dm_settings(Actor::new(actor))
            .await
            .map_err(map_service_error)?
            .into())
    }
}
