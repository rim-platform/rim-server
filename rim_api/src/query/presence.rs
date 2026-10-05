use async_graphql::*;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::UserPresenceGql;

#[derive(Default)]
pub(crate) struct PresenceQueries;

#[Object]
impl PresenceQueries {
    async fn workspace_presence(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
    ) -> Result<Vec<UserPresenceGql>> {
        let actor = require_actor(ctx)?;
        let presence_mgr =
            ctx.data::<std::sync::Arc<dyn rim_services::presence::PresenceService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;
        crate::auth_context::require_server_member(ctx, actor, workspace_id).await?;

        let entries = presence_mgr.get_workspace_presence(workspace_id);
        Ok(entries.into_iter().map(Into::into).collect())
    }
}
