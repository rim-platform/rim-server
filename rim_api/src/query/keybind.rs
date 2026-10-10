use async_graphql::*;
use rim_services::Actor;
use rim_services::keybind::request::ResolveKeybindsQuery;
use rim_services::keybind::service::KeybindService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::Keybind;

#[derive(Default)]
pub(crate) struct KeybindQueries;

#[Object]
impl KeybindQueries {
    async fn resolved_keybinds(
        &self,
        ctx: &Context<'_>,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .resolve_keybinds(ResolveKeybindsQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }
}
