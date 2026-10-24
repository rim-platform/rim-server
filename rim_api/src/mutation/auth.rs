use async_graphql::*;
use rim_services::Actor;
use rim_services::auth::command::{LogoutCmd, LogoutOtherSessionsCmd};
use rim_services::auth::service::AuthService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;

#[derive(Default)]
pub(crate) struct AuthMutations;

#[Object]
impl AuthMutations {
    async fn logout(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.logout(LogoutCmd {
            actor: Actor::new(actor),
            session_id,
        })
        .await
        .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::ClearSession);
        }
        Ok(true)
    }

    /// End every session of the signed-in account except this one. Returns how many ended.
    async fn logout_other_sessions(&self, ctx: &Context<'_>) -> Result<i64> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let ended = auth
            .logout_other_sessions(LogoutOtherSessionsCmd {
                actor: Actor::new(actor),
                session_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(i64::try_from(ended).unwrap_or(i64::MAX))
    }
}

/// Payload returned from `loginWithKey`.
#[derive(SimpleObject)]
pub struct LoginPayload {
    pub user_id: ID,
    /// The new session's CSRF token, the value of the `rim_csrf` cookie. A client that cannot
    /// read that cookie keeps it in memory and sends it in the `X-CSRF-Token` header.
    pub csrf_token: String,
    /// After `loginWithKey` without `keepSignedIn`: the session secret, unpadded base64url, in
    /// the clear. The client keeps it in memory only and sends it in the
    /// `X-RIM-Session-Secret` header of every request and in the WebSocket's
    /// `connection_init` payload as `sessionSecret`. `null` for a kept session.
    pub session_secret: Option<String>,
    /// After `loginWithKey` with `keepSignedIn` and without `keepKeyHeld`: the keep key the
    /// server holds for this session, 32 bytes as unpadded base64url, in the clear. `null`
    /// otherwise.
    pub keep_key: Option<String>,
}
