use async_graphql::*;
use rim_services::Actor;
use rim_services::key_auth::command::GetMyKeyIdentityQuery;
use rim_services::key_auth::service::KeyAuthService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::KeyIdentity;

#[derive(Default)]
pub(crate) struct KeyIdentityQueries;

#[Object]
impl KeyIdentityQueries {
    /// The signed-in account's key log, devices and profile card on this server.
    async fn my_key_identity(&self, ctx: &Context<'_>) -> Result<KeyIdentity> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .get_my_key_identity(GetMyKeyIdentityQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }
}
