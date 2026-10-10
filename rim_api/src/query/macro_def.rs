use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::MacroDef;

#[derive(Default)]
pub(crate) struct MacroQueries;

#[Object]
impl MacroQueries {
    // ── Macros ──────────────────────────────────────────────────────────

    async fn my_macros(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<MacroDef>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::macro_def::service::MacroService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let macros = svc
            .list_user_macros(rim_services::macro_def::request::ListUserMacrosQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(macros.into_iter().map(Into::into).collect())
    }

    async fn macro_by_name(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
    ) -> Result<Option<MacroDef>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::macro_def::service::MacroService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let macro_def = svc
            .get_macro_by_name(rim_services::macro_def::request::GetMacroByNameQuery {
                actor: Actor::new(actor),
                server_id,
                name,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(macro_def.map(Into::into))
    }
}
