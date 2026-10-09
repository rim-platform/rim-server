use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::{require_actor, require_mls_caller};
use crate::errors::map_service_error;
use crate::federation::relay;
use crate::ids::parse_uuid_id;
use crate::query::relay::parse_item_id;
use crate::types::{Contact, DmRequest};

#[derive(Default)]
pub(crate) struct RelayMutations;

#[Object]
impl RelayMutations {
    /// Add a contact, or replace its relay URLs (1 to 8 of at most 256 bytes each).
    async fn add_contact(
        &self,
        ctx: &Context<'_>,
        principal_id: String,
        relay_urls: Vec<String>,
    ) -> Result<Contact> {
        let actor = require_actor(ctx)?;
        Ok(relay(ctx)?
            .relay
            .add_contact(Actor::new(actor), principal_id, relay_urls)
            .await
            .map_err(map_service_error)?
            .into())
    }

    async fn remove_contact(&self, ctx: &Context<'_>, principal_id: String) -> Result<bool> {
        let actor = require_actor(ctx)?;
        relay(ctx)?
            .relay
            .remove_contact(Actor::new(actor), principal_id)
            .await
            .map_err(map_service_error)
    }

    /// Release the request's held items, let later ones flow, and add the sender as a contact.
    async fn accept_dm_request(&self, ctx: &Context<'_>, id: ID) -> Result<DmRequest> {
        let actor = require_actor(ctx)?;
        let id = parse_uuid_id::<rim_core::relay::model::DmRequestId>(&id)?;
        Ok(relay(ctx)?
            .relay
            .accept_dm_request(Actor::new(actor), id)
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Delete the request's held items and drop later items of the conversation.
    async fn ignore_dm_request(&self, ctx: &Context<'_>, id: ID) -> Result<DmRequest> {
        let actor = require_actor(ctx)?;
        let id = parse_uuid_id::<rim_core::relay::model::DmRequestId>(&id)?;
        Ok(relay(ctx)?
            .relay
            .ignore_dm_request(Actor::new(actor), id)
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Ignore a principal's pending requests and refuse its invites and key package claims.
    async fn block_principal(&self, ctx: &Context<'_>, principal_id: String) -> Result<bool> {
        let actor = require_actor(ctx)?;
        relay(ctx)?
            .relay
            .block_principal(Actor::new(actor), principal_id)
            .await
            .map_err(map_service_error)
    }

    async fn unblock_principal(&self, ctx: &Context<'_>, principal_id: String) -> Result<bool> {
        let actor = require_actor(ctx)?;
        relay(ctx)?
            .relay
            .unblock_principal(Actor::new(actor), principal_id)
            .await
            .map_err(map_service_error)
    }

    /// Report a DM request as abuse; a pending one is ignored too.
    async fn report_dm_request(&self, ctx: &Context<'_>, id: ID, reason: String) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let id = parse_uuid_id::<rim_core::relay::model::DmRequestId>(&id)?;
        relay(ctx)?
            .relay
            .report_dm_request(Actor::new(actor), id, reason)
            .await
            .map_err(map_service_error)
    }

    /// Move the session device's cursor up to `upTo`.
    async fn ack_relay_items(&self, ctx: &Context<'_>, up_to: ID) -> Result<bool> {
        let caller = require_mls_caller(ctx)?;
        let up_to = parse_item_id(&up_to)?;
        relay(ctx)?
            .relay
            .ack_relay_items(caller, up_to)
            .await
            .map_err(map_service_error)
    }

    /// Delete the caller's queue, requests, contacts and blocks on this relay.
    async fn leave_relay(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = require_actor(ctx)?;
        relay(ctx)?
            .relay
            .leave_relay(Actor::new(actor))
            .await
            .map_err(map_service_error)
    }
}
