use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct WorkspaceStateMutations;

#[Object]
impl WorkspaceStateMutations {
    async fn set_workspace_state(
        &self,
        ctx: &Context<'_>,
        input: crate::types::SetWorkspaceStateInput,
    ) -> Result<crate::types::WorkspaceStateEntryGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let entry = svc
            .set(
                rim_services::workspace_state::request::SetWorkspaceStateCmd {
                    actor: Actor::new(actor),
                    server_id,
                    key: input.key,
                    value: input.value.0,
                    scope: input.scope.into(),
                    ttl_seconds: input.ttl_seconds,
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(entry.into())
    }

    async fn delete_workspace_state(
        &self,
        ctx: &Context<'_>,
        input: crate::types::DeleteWorkspaceStateInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let deleted = svc
            .delete(
                rim_services::workspace_state::request::DeleteWorkspaceStateCmd {
                    actor: Actor::new(actor),
                    server_id,
                    key: input.key,
                    scope: input.scope.into(),
                },
            )
            .await
            .map_err(map_service_error)?;

        Ok(deleted)
    }
}
