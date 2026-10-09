use async_graphql::*;
use rim_services::Actor;
use rim_services::device_link::command::{GetDeviceLinkAnswerQuery, GetDeviceLinkQuery};
use rim_services::device_link::service::DeviceLinkService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::{DeviceLink, SealedMessage};

#[derive(Default)]
pub(crate) struct DeviceLinkQueries;

#[Object]
impl DeviceLinkQueries {
    /// One of the signed-in account's device links: its state, expiry and, once the new device
    /// sent it, the sealed offer.
    async fn device_link(&self, ctx: &Context<'_>, link_id: String) -> Result<DeviceLink> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        let dto = svc
            .get(GetDeviceLinkQuery {
                actor: Actor::new(actor),
                link_id,
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// The sealed answer for the new device, `null` until the old device sends it. No session
    /// needed: the link id is the capability.
    async fn device_link_answer(
        &self,
        ctx: &Context<'_>,
        link_id: String,
    ) -> Result<Option<SealedMessage>> {
        let svc = ctx.data::<std::sync::Arc<dyn DeviceLinkService>>()?;
        let answer = svc
            .get_answer(GetDeviceLinkAnswerQuery {
                link_id,
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;
        Ok(answer.map(Into::into))
    }
}
