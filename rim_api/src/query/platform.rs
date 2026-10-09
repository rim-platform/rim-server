use async_graphql::*;
use rim_services::Actor;
use rim_services::capability::service::CapabilityService;

use crate::auth_context::require_actor;
use crate::types::Capability;

#[derive(Default)]
pub(crate) struct PlatformQueries;

#[Object]
impl PlatformQueries {
    async fn capabilities(&self, ctx: &Context<'_>) -> Result<Vec<Capability>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CapabilityService>>()?;

        let capabilities = svc
            .list_capabilities(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(capabilities.into_iter().map(Into::into).collect())
    }
}
