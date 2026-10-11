use async_graphql::*;
use rim_services::auth::service::AuthService;

use crate::types::{InviteTokenPreviewGql, RegistrationModeGql};

#[derive(Default)]
pub(crate) struct RegistrationQueries;

#[Object]
impl RegistrationQueries {
    /// Return the deployment-wide registration mode.
    ///
    /// Used by the login page to decide whether to render a "Sign up" link.
    /// No authentication required — this is public configuration metadata.
    async fn registration_mode(&self, ctx: &Context<'_>) -> Result<RegistrationModeGql> {
        let mode = ctx.data::<std::sync::Arc<rim_services::auth::dto::RegistrationMode>>()?;
        Ok(RegistrationModeGql::from(mode.as_ref().clone()))
    }

    /// Preview a registration token without consuming it.
    ///
    /// Returns minimal metadata so the frontend can render the correct signup
    /// copy before the user submits credentials. Rate-limited at 30 req / 5 min
    /// per IP; no authentication required.
    async fn preview_invite_token(
        &self,
        ctx: &Context<'_>,
        token: String,
    ) -> Result<InviteTokenPreviewGql> {
        let remote_ip = crate::auth_context::remote_ip(ctx);

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let preview = auth
            .preview_invite_token(rim_services::auth::command::PreviewInviteTokenQuery {
                token,
                remote_ip,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(InviteTokenPreviewGql::from(preview))
    }
}
