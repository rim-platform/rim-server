use async_graphql::*;
use rim_core::server::model::ServerId;
use rim_services::Actor;
use rim_services::workspace_plugin::command::ListWorkspacePluginsQuery;
use rim_services::workspace_plugin::service::WorkspacePluginService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::WorkspacePlugin;

#[derive(Default)]
pub(crate) struct WorkspacePluginQueries;

#[Object]
impl WorkspacePluginQueries {
    /// The workspace's plugin registry, ordered by plugin id. Any member, guests included.
    async fn workspace_plugins(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<WorkspacePlugin>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn WorkspacePluginService>>()?;
        let entries = svc
            .list(ListWorkspacePluginsQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id::<ServerId>(&server_id)?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(entries.into_iter().map(Into::into).collect())
    }
}
