use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::{optional_actor, require_actor, require_mls_caller};
use crate::errors::map_service_error;
use crate::federation::{federation, relay};
use crate::ids::parse_uuid_id;
use crate::types::{
    Contact, DmRequest, DmRequestStatus, RelayInfo, RelayItemPage, RemoteIdentity, RemoteServer,
    StreamHead,
};

/// Default and largest `relayItems` page.
const DEFAULT_RELAY_ITEMS: i32 = 200;
const MAX_RELAY_ITEMS: i32 = 500;

/// Parse an item id given as a GraphQL `ID`.
pub(crate) fn parse_item_id(id: &ID) -> Result<i64> {
    id.parse::<i64>()
        .ok()
        .filter(|id| *id >= 0)
        .ok_or_else(|| Error::new("an item id is a decimal number"))
}

#[derive(Default)]
pub(crate) struct RelayQueries;

#[Object]
impl RelayQueries {
    /// This server's id, roles and direct space. Readable without a session; a signed-in local
    /// user of a relay becomes a guest of the direct space, if not one yet.
    async fn relay_info(&self, ctx: &Context<'_>) -> Result<RelayInfo> {
        let federation = federation(ctx)?;
        if let (Some(actor), Ok(relay)) = (optional_actor(ctx), relay(ctx)) {
            relay
                .host
                .join_direct_space(actor)
                .await
                .map_err(map_service_error)?;
        }
        Ok(federation.info.clone().into())
    }

    /// Pin the server at `origin` through this relay and return its discovery document.
    async fn remote_server(&self, ctx: &Context<'_>, origin: String) -> Result<RemoteServer> {
        let actor = require_actor(ctx)?;
        let relay = relay(ctx)?;
        Ok(relay
            .relay
            .remote_server(Actor::new(actor), origin)
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// A principal's identity: a local one directly, else fetched from `relayUrl` (pinning it)
    /// and cached for 24 hours.
    async fn remote_identity(
        &self,
        ctx: &Context<'_>,
        principal_id: String,
        relay_url: Option<String>,
    ) -> Result<RemoteIdentity> {
        let actor = require_actor(ctx)?;
        let relay = relay(ctx)?;
        Ok(relay
            .relay
            .remote_identity(Actor::new(actor), principal_id, relay_url)
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// The caller's contacts, oldest first.
    async fn contacts(&self, ctx: &Context<'_>) -> Result<Vec<Contact>> {
        let actor = require_actor(ctx)?;
        let relay = relay(ctx)?;
        Ok(relay
            .relay
            .contacts(Actor::new(actor))
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// The caller's DM requests, newest first.
    async fn dm_requests(
        &self,
        ctx: &Context<'_>,
        status: Option<DmRequestStatus>,
    ) -> Result<Vec<DmRequest>> {
        let actor = require_actor(ctx)?;
        let relay = relay(ctx)?;
        Ok(relay
            .relay
            .dm_requests(Actor::new(actor), status.map(Into::into))
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// The session device's queued host events above `after` (its cursor when omitted).
    /// `limit` is 1 to 500, 200 by default.
    async fn relay_items(
        &self,
        ctx: &Context<'_>,
        after: Option<ID>,
        limit: Option<i32>,
    ) -> Result<RelayItemPage> {
        let caller = require_mls_caller(ctx)?;
        let relay = relay(ctx)?;
        let limit = limit.unwrap_or(DEFAULT_RELAY_ITEMS);
        if !(1..=MAX_RELAY_ITEMS).contains(&limit) {
            return Err(Error::new(format!("limit is 1 to {MAX_RELAY_ITEMS}")));
        }
        let after = after.as_ref().map(parse_item_id).transpose()?;
        Ok(relay
            .relay
            .relay_items(caller, after, i64::from(limit))
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// The current `seq` of each named event stream of a space hosted here that the caller can
    /// see; streams the caller cannot see are left out.
    async fn stream_heads(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        streams: Vec<String>,
    ) -> Result<Vec<StreamHead>> {
        let actor = require_actor(ctx)?;
        let federation = federation(ctx)?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        Ok(federation
            .host
            .stream_heads(Actor::new(actor), server_id, streams)
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}
