use async_graphql::*;
use rim_services::Actor;
use rim_services::server_member::command::ListServerMembersQuery;
use rim_services::server_member::service::ServerMemberService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::UserPresenceGql;

#[derive(Default)]
pub(crate) struct PresenceQueries;

#[Object]
impl PresenceQueries {
    /// Presence of the workspace's members who sent a heartbeat. A guest sees only the members
    /// they share a conversation with, as in the member directory.
    async fn workspace_presence(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
    ) -> Result<Vec<UserPresenceGql>> {
        let actor = require_actor(ctx)?;
        let presence_mgr =
            ctx.data::<std::sync::Arc<dyn rim_services::presence::PresenceService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;
        let visible = ctx
            .data::<std::sync::Arc<dyn ServerMemberService>>()?
            .visible_members(ListServerMembersQuery {
                actor: Actor::new(actor),
                server_id: workspace_id,
            })
            .await
            .map_err(map_service_error)?;

        let entries = presence_mgr.get_workspace_presence(workspace_id);
        Ok(entries
            .into_iter()
            .filter(|entry| {
                visible
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&entry.user_id))
            })
            .map(Into::into)
            .collect())
    }
}
