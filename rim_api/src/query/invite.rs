use async_graphql::*;
use rim_services::Actor;
use rim_services::auth::command::ListInvitesQuery;
use rim_services::auth::service::AuthService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::{InstanceInviteGql, Invite, InviteTokenPreviewGql, RegistrationModeGql};

#[derive(Default)]
pub(crate) struct InviteQueries;

#[Object]
impl InviteQueries {
    async fn my_invites(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<Invite>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let invites = svc
            .list_invites(ListInvitesQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(invites.into_iter().map(Into::into).collect())
    }
}

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

    /// List all active (non-revoked) instance invites.
    ///
    /// Admin-only: requires the caller to be `Owner` on at least one workspace.
    /// Returns `FORBIDDEN` otherwise.
    async fn list_instance_invites(&self, ctx: &Context<'_>) -> Result<Vec<InstanceInviteGql>> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let invites = auth
            .list_instance_invites(rim_services::Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(invites.into_iter().map(InstanceInviteGql::from).collect())
    }

    /// Preview a registration token without consuming it.
    ///
    /// Returns minimal metadata so the frontend can render the correct signup
    /// copy before the user submits credentials. Rate-limited at 30 req / 5 min
    /// per IP; no authentication required.
    ///
    /// See spec §4.7.
    async fn preview_invite_token(
        &self,
        ctx: &Context<'_>,
        token: String,
    ) -> Result<InviteTokenPreviewGql> {
        // Marker token: the rate limiter is internal to `DefaultAuthService` and is
        // always present — "missing limiter" is a state the type system makes
        // unrepresentable (spec §4.7 R2). This context lookup is kept as a
        // proof-of-wiring artifact; the marker is wired at schema-build time in
        // `RegistrationContext`. Nit1 hardening: comment updated — limiter is
        // internal; context just carries this marker token.
        let _ = ctx
            .data::<std::sync::Arc<rim_services::auth::dto::PreviewInviteRateLimiterToken>>()
            .map_err(|_| {
                async_graphql::Error::new("internal error: preview rate limiter not configured")
                    .extend_with(|_, e| e.set("code", "INTERNAL"))
            })?;

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
