use async_graphql::*;
use rim_services::Actor;
use rim_services::auth::command::ListInvitesQuery;
use rim_services::auth::service::AuthService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    InviteTokenPreviewGql, RegistrationModeGql, WorkspaceInviteCursorInput, WorkspaceInvitePage,
};

/// Page size of `workspaceInvites` when `limit` is omitted.
const DEFAULT_LIMIT: i32 = 50;
/// Largest page `workspaceInvites` returns.
const MAX_LIMIT: u32 = 100;

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

#[derive(Default)]
pub(crate) struct WorkspaceInviteQueries;

#[Object]
impl WorkspaceInviteQueries {
    /// The workspace's invites, newest first (`createdAt DESC, id DESC`), revoked, expired and
    /// used-up ones included. `AFTER` pages go to older invites. Defaults: `dir` `AFTER`,
    /// `limit` 50 (clamped to 1..=100). Owner only: anyone else gets `FORBIDDEN`. An invite's
    /// token is never listed.
    async fn workspace_invites(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: Option<PageDir>,
        cursor: Option<WorkspaceInviteCursorInput>,
        limit: Option<i32>,
    ) -> Result<WorkspaceInvitePage> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let limit = u32::try_from(limit.unwrap_or(DEFAULT_LIMIT).max(1))
            .unwrap_or(1)
            .min(MAX_LIMIT);
        let page = auth
            .list_invites(ListInvitesQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                dir: dir.unwrap_or(PageDir::After).into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit,
            })
            .await
            .map_err(map_service_error)?;
        Ok(WorkspaceInvitePage::from_page(page))
    }
}
