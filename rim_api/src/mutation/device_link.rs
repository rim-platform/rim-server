use async_graphql::*;
use rim_services::Actor;
use rim_services::device_link::command::{
    CancelDeviceLinkCmd, CompleteDeviceLinkCmd, SendDeviceLinkAnswerCmd, SendDeviceLinkOfferCmd,
    StartDeviceLinkCmd,
};
use rim_services::device_link::service::DeviceLinkService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::{DeviceLink, SealedMessageInput, StartedDeviceLink};

#[derive(Default)]
pub(crate) struct DeviceLinkMutations;

#[Object]
impl DeviceLinkMutations {
    /// Start linking a new device to the signed-in account. Cancels the account's previous
    /// live link. The link lives 10 minutes.
    async fn start_device_link(&self, ctx: &Context<'_>) -> Result<StartedDeviceLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        let dto = svc
            .start(StartDeviceLinkCmd {
                actor: Actor::new(actor),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// The new device's sealed offer. No session needed: the link id is the capability.
    async fn send_device_link_offer(
        &self,
        ctx: &Context<'_>,
        link_id: String,
        offer: SealedMessageInput,
    ) -> Result<bool> {
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        svc.send_offer(SendDeviceLinkOfferCmd {
            link_id,
            offer: offer.into(),
            remote_ip: crate::auth_context::remote_ip(ctx),
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }

    /// The old device's sealed answer, carrying the new device's certificate. The link then
    /// lives another 10 minutes.
    async fn send_device_link_answer(
        &self,
        ctx: &Context<'_>,
        link_id: String,
        answer: SealedMessageInput,
    ) -> Result<DeviceLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        let dto = svc
            .send_answer(SendDeviceLinkAnswerCmd {
                actor: Actor::new(actor),
                link_id,
                answer: answer.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Called by the new device right after its first `loginWithKey`.
    async fn complete_device_link(&self, ctx: &Context<'_>, link_id: String) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        svc.complete(CompleteDeviceLinkCmd {
            actor: Actor::new(actor),
            link_id,
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }

    /// Cancel a link from either device, with or without a session.
    async fn cancel_device_link(&self, ctx: &Context<'_>, link_id: String) -> Result<bool> {
        let actor = crate::auth_context::optional_actor(ctx);
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        svc.cancel(CancelDeviceLinkCmd {
            actor: actor.map(Actor::new),
            link_id,
            remote_ip: crate::auth_context::remote_ip(ctx),
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }
}
