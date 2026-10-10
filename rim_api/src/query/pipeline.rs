use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct PipelineQueries;

#[Object]
impl PipelineQueries {
    async fn pipelines(
        &self,
        ctx: &Context<'_>,
        server_id: async_graphql::ID,
        enabled: Option<bool>,
    ) -> Result<Vec<crate::types::PipelineGql>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let pipelines = svc
            .list(rim_services::pipeline::request::ListPipelinesCmd {
                actor: Actor::new(actor),
                server_id,
                enabled,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(pipelines.into_iter().map(Into::into).collect())
    }
}
