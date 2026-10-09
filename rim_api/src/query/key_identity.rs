use async_graphql::*;
use rim_services::Actor;
use rim_services::key_auth::command::{GetKeyLogQuery, GetMyKeyIdentityQuery};
use rim_services::key_auth::service::KeyAuthService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::{KeyIdentity, KeyLogEntry};

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

    /// The public key log of a principal enrolled on this server, oldest first; empty when none
    /// is. No session needed: a person who lost every device reads the head here to sign a
    /// recovery rotation (`rotateIdentityKey`).
    async fn key_log(&self, ctx: &Context<'_>, principal_id: String) -> Result<Vec<KeyLogEntry>> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let entries = svc
            .get_key_log(GetKeyLogQuery { principal_id })
            .await
            .map_err(map_service_error)?;
        Ok(entries.into_iter().map(Into::into).collect())
    }
}
