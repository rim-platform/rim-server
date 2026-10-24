use async_graphql::*;
use rim_services::session_key::service::SessionKeyService;

use crate::auth_context::{require_actor, require_session_id};
use crate::errors::map_service_error;
use crate::types::ReloadSessionPayload;
use crate::types::account_password::encode_bytes;

#[derive(Default)]
pub(crate) struct SessionKeyMutations;

#[Object]
impl SessionKeyMutations {
    /// Make a new reload key for this session, which was signed in without "Keep me signed
    /// in", and return it, 32 bytes as unpadded base64url. The tab seals what it stores in
    /// `sessionStorage` under it, right after signing in and after each reload. The server
    /// keeps only this key; the previous one stops opening anything. `UNAUTHENTICATED` for a
    /// kept session.
    async fn set_reload_key(&self, ctx: &Context<'_>) -> Result<String> {
        require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn SessionKeyService>>()?;
        svc.set_reload_key(session_id)
            .await
            .map(|key| encode_bytes(&key))
            .map_err(map_service_error)
    }

    /// After a reload, hand this session's tab the reload key its stored data is sealed under,
    /// and a new one the server keeps instead. It needs the session cookie and the CSRF header,
    /// but not the session secret, which the tab lost in the reload. It answers once, and only
    /// while the session is unkept and no WebSocket of it is open, within 60 seconds of its
    /// WebSocket closing; otherwise `UNAUTHENTICATED`, and the account is locked on this
    /// device.
    async fn reload_session(&self, ctx: &Context<'_>) -> Result<ReloadSessionPayload> {
        require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn SessionKeyService>>()?;
        svc.reload(session_id)
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }
}
