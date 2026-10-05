use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct WorkspaceFeatureMutations;

#[Object]
impl WorkspaceFeatureMutations {
    // ── Workspace features (Track 0.4) ────────────────────────────────

    /// Turn an optional workspace module on or off. Owner-only. No data is deleted when a
    /// feature is turned off; turning it back on restores everything. Audited.
    async fn set_workspace_feature(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        feature: crate::types::WorkspaceFeature,
        enabled: bool,
    ) -> Result<crate::types::WorkspaceFeatureState> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<
                dyn rim_services::workspace_feature::service::WorkspaceFeatureService,
            >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let state = svc
            .set(
                rim_services::workspace_feature::command::SetWorkspaceFeatureCmd {
                    actor: Actor::new(actor),
                    server_id,
                    feature: feature.into(),
                    enabled,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(state.into())
    }
}
