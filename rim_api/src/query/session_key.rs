use async_graphql::*;
use rim_services::session_key::service::SessionKeyService;

use crate::auth_context::{require_actor, require_session_id};
use crate::errors::map_service_error;
use crate::types::account_password::encode_bytes;

#[derive(Default)]
pub(crate) struct SessionKeyQueries;

#[Object]
impl SessionKeyQueries {
    /// The keep key the server holds for this session, 32 bytes as unpadded base64url, in the
    /// clear: what a web client signed in with "Keep me signed in" opens its data key with at
    /// start. `null` for a session signed in without it, and for one whose client holds its
    /// own keep key.
    async fn session_keep_key(&self, ctx: &Context<'_>) -> Result<Option<String>> {
        require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn SessionKeyService>>()?;
        svc.keep_key(session_id)
            .await
            .map(|key| key.as_deref().map(encode_bytes))
            .map_err(map_service_error)
    }
}
