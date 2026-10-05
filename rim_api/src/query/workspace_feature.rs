use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct WorkspaceFeatureQueries;

#[Object]
impl WorkspaceFeatureQueries {
    /// Per-workspace optional modules and whether each is on (Track 0.4). Any member; one
    /// entry per known feature, defaults filled in.
    async fn workspace_features(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<crate::types::WorkspaceFeatureState>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<
                dyn rim_services::workspace_feature::service::WorkspaceFeatureService,
            >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let states = svc
            .list(
                rim_services::workspace_feature::command::ListWorkspaceFeaturesQuery {
                    actor: Actor::new(actor),
                    server_id,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(states.into_iter().map(Into::into).collect())
    }
}
