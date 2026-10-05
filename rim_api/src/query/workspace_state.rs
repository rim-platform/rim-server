use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct WorkspaceStateQueries;

#[Object]
impl WorkspaceStateQueries {
    async fn workspace_state(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        key: String,
        scope: crate::types::WorkspaceStateScopeGql,
    ) -> Result<Option<crate::types::WorkspaceStateEntryGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let entry = svc
            .get(
                rim_services::workspace_state::request::GetWorkspaceStateQuery {
                    actor: Actor::new(actor),
                    server_id,
                    key,
                    scope: scope.into(),
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(entry.map(Into::into))
    }

    async fn workspace_state_list(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        prefix: String,
        scope: crate::types::WorkspaceStateScopeGql,
        limit: Option<u64>,
    ) -> Result<Vec<crate::types::WorkspaceStateEntryGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let entries = svc
            .list(
                rim_services::workspace_state::request::ListWorkspaceStateQuery {
                    actor: Actor::new(actor),
                    server_id,
                    prefix,
                    scope: scope.into(),
                    limit,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(entries.into_iter().map(Into::into).collect())
    }
}
