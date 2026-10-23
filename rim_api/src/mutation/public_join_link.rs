use async_graphql::*;
use rim_services::Actor;
use rim_services::public_join_link::dto::PublicJoinLinkPatch;
use rim_services::public_join_link::service::PublicJoinLinkService;

use crate::auth_context::{remote_ip, require_actor};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    JoinByPublicLinkPayload, JoinRequest, PublicJoinLink, UpdatePublicJoinLinkInput,
};

#[derive(Default)]
pub(crate) struct PublicJoinLinkMutations;

#[Object]
impl PublicJoinLinkMutations {
    /// Change the workspace's public link: on or off, `mode`, `role` (`MEMBER` or `GUEST`) and
    /// `allowRemote`. Turning it on for the first time issues its token; turning it off makes
    /// every lookup of it answer `NOT_FOUND` until it is turned on again. Owner only
    /// (`FORBIDDEN`); `VALIDATION` for the direct space, the server's default workspace and any
    /// other role.
    async fn update_public_join_link(
        &self,
        ctx: &Context<'_>,
        input: UpdatePublicJoinLinkInput,
    ) -> Result<PublicJoinLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.update_public_join_link(
            Actor::new(actor),
            parse_uuid_id(&input.server_id)?,
            PublicJoinLinkPatch {
                enabled: input.enabled,
                mode: input.mode.map(Into::into),
                role: input.role.map(Into::into),
                allow_remote: input.allow_remote,
            },
        )
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Issue a new token for the workspace's public link; the old one stops working at once.
    /// The settings stay. Owner only, with the refusals of `updatePublicJoinLink`.
    async fn reissue_public_join_link(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<PublicJoinLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.reissue_public_join_link(Actor::new(actor), parse_uuid_id(&server_id)?)
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }

    /// Join a workspace through its public link (`DIRECT`), or ask to join with an optional
    /// `message` of at most 500 characters (`REQUESTS`). For an account of this server; the
    /// link never creates an account. An invalid link is `NOT_FOUND` and counts towards the
    /// address's misses (`RATE_LIMITED` past them); a ban is `FORBIDDEN`; a full workspace,
    /// a too long message, 20 open requests or a decline less than 24 hours ago are
    /// `VALIDATION`; a member or an open request is `CONFLICT`.
    async fn join_by_public_link(
        &self,
        ctx: &Context<'_>,
        token: String,
        message: Option<String>,
    ) -> Result<JoinByPublicLinkPayload> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.join(Actor::new(actor), token, message, remote_ip(ctx))
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }

    /// Take back the caller's open join request. `NOT_FOUND` unless it is theirs, `CONFLICT`
    /// once it is no longer open.
    async fn withdraw_join_request(
        &self,
        ctx: &Context<'_>,
        request_id: ID,
    ) -> Result<JoinRequest> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.withdraw_join_request(Actor::new(actor), parse_uuid_id(&request_id)?)
            .await
            .map(Into::into)
            .map_err(map_service_error)
    }

    /// Accept an open join request: the person joins with the link's current role and the
    /// public channels, as a direct join does; a person who has joined another way since is
    /// left as they are and the request is closed as accepted. Owner and Moderators only
    /// (`FORBIDDEN`); a request of another workspace is `NOT_FOUND`, one no longer open is
    /// `CONFLICT`, a ban `FORBIDDEN`, a full workspace `VALIDATION`.
    async fn accept_join_request(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        request_id: ID,
    ) -> Result<JoinRequest> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.accept_join_request(
            Actor::new(actor),
            parse_uuid_id(&server_id)?,
            parse_uuid_id(&request_id)?,
        )
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Decline an open join request; the person may ask again 24 hours later. Owner and
    /// Moderators only, with the refusals of `acceptJoinRequest` other than the join checks.
    async fn decline_join_request(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        request_id: ID,
    ) -> Result<JoinRequest> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn PublicJoinLinkService>>()?;
        svc.decline_join_request(
            Actor::new(actor),
            parse_uuid_id(&server_id)?,
            parse_uuid_id(&request_id)?,
        )
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }
}
