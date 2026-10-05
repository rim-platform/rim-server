use async_graphql::*;
use rim_services::Actor;
use rim_services::keybind::request::{
    InspectKeybindLayersQuery, ListDefaultKeybindsQuery, ListKeybindsQuery, ResolveKeybindQuery,
    ResolveKeybindsQuery,
};
use rim_services::keybind::service::KeybindService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::{DefaultKeybind, Keybind, KeybindContext, KeybindLayers, KeybindScope};

#[derive(Default)]
pub(crate) struct KeybindQueries;

#[Object]
impl KeybindQueries {
    async fn keybinds(
        &self,
        ctx: &Context<'_>,
        scope: KeybindScope,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .list_keybinds(ListKeybindsQuery {
                actor: Actor::new(actor),
                scope: scope.into(),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }

    async fn default_keybinds(&self, ctx: &Context<'_>) -> Result<Vec<DefaultKeybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .list_default_keybinds(ListDefaultKeybindsQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }

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

    async fn resolve_keybind(
        &self,
        ctx: &Context<'_>,
        keystroke: String,
        context: KeybindContext,
        workspace_id: Option<ID>,
    ) -> Result<Option<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybind = svc
            .resolve_keybind(ResolveKeybindQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke,
                context: context.into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybind.map(Into::into))
    }

    async fn keybind_layers(
        &self,
        ctx: &Context<'_>,
        keystroke: String,
        context: KeybindContext,
        workspace_id: Option<ID>,
    ) -> Result<KeybindLayers> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let layers = svc
            .inspect_keybind_layers(InspectKeybindLayersQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke,
                context: context.into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(layers.into())
    }
}
