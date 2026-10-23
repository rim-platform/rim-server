use async_graphql::*;
use rim_services::Actor;
use rim_services::public_join_link::service::{ListJoinRequestsQuery, PublicJoinLinkService};

use crate::auth_context::{optional_actor, remote_ip, require_actor};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    JoinRequest, JoinRequestCursorInput, JoinRequestPage, PublicJoinLink, PublicJoinLinkPreview,
};

/// Page size of `joinRequests` when `limit` is omitted.
const DEFAULT_LIMIT: i32 = 50;
/// Largest page `joinRequests` returns.
const MAX_LIMIT: u32 = 100;

#[derive(Default)]
pub(crate) struct PublicJoinLinkQueries;

#[Object]
impl PublicJoinLinkQueries {
    /// The workspace's public link, its settings and how it is used, for its Owner (`FORBIDDEN`
    /// for anyone else; `VALIDATION` for the direct space and the server's default workspace).
    /// A workspace that never had a link answers with `enabled` false and a null `token`.
    async fn public_join_link(&self, ctx: &Context<'_>, server_id: ID) -> Result<PublicJoinLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.public_join_link(Actor::new(actor), parse_uuid_id(&server_id)?)
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }

    /// What a valid, switched-on public link shows; no sign-in needed. Every other token
    /// answers `NOT_FOUND` with the same message, and an address with 20 such answers in ten
    /// minutes gets `RATE_LIMITED` for every link lookup and join for an hour.
    async fn public_join_link_preview(
        &self,
        ctx: &Context<'_>,
        token: String,
    ) -> Result<PublicJoinLinkPreview> {
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.preview(optional_actor(ctx).map(Actor::new), token, remote_ip(ctx))
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }

    /// The workspace's open join requests, newest first (`createdAt DESC, id DESC`); `AFTER`
    /// pages go to older ones. Defaults: `dir` `AFTER`, `limit` 50 (clamped to 1..=100). Owner
    /// and Moderators only: anyone else gets `FORBIDDEN`.
    async fn join_requests(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: Option<PageDir>,
        cursor: Option<JoinRequestCursorInput>,
        limit: Option<i32>,
    ) -> Result<JoinRequestPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        let limit = u32::try_from(limit.unwrap_or(DEFAULT_LIMIT).max(1))
            .unwrap_or(1)
            .min(MAX_LIMIT);
        let page = svc
            .list_join_requests(ListJoinRequestsQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                dir: dir.unwrap_or(PageDir::After).into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit,
            })
            .await
            .map_err(map_service_error)?;
        Ok(JoinRequestPage::from_page(page))
    }

    /// The caller's open join requests, newest first; at most 20.
    async fn my_join_requests(&self, ctx: &Context<'_>) -> Result<Vec<JoinRequest>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        let requests = svc
            .my_join_requests(Actor::new(actor))
            .await
            .map_err(map_service_error)?;
        Ok(requests.into_iter().map(Into::into).collect())
    }
}
