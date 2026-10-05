use async_graphql::*;
use rim_services::Actor;
use rim_services::action::service::ActionService;
use rim_services::capability::service::CapabilityService;
use rim_services::event::service::EventService;
use rim_services::integration::service::IntegrationService;
use rim_services::setting::service::SettingService;

use crate::auth_context::require_actor;
use crate::types::{Action, Capability, Event, Integration, Setting};

#[derive(Default)]
pub(crate) struct PlatformQueries;

#[Object]
impl PlatformQueries {
    async fn actions(&self, ctx: &Context<'_>) -> Result<Vec<Action>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ActionService>>()?;

        let actions = svc
            .list_actions(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(actions.into_iter().map(Into::into).collect())
    }

    async fn events(&self, ctx: &Context<'_>) -> Result<Vec<Event>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn EventService>>()?;

        let events = svc
            .list_events(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(events.into_iter().map(Into::into).collect())
    }

    async fn capabilities(&self, ctx: &Context<'_>) -> Result<Vec<Capability>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CapabilityService>>()?;

        let capabilities = svc
            .list_capabilities(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(capabilities.into_iter().map(Into::into).collect())
    }

    async fn integrations(&self, ctx: &Context<'_>) -> Result<Vec<Integration>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn IntegrationService>>()?;

        let integrations = svc
            .list_integrations(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(integrations.into_iter().map(Into::into).collect())
    }

    async fn settings(&self, ctx: &Context<'_>) -> Result<Vec<Setting>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn SettingService>>()?;

        let settings = svc
            .list_settings(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(settings.into_iter().map(Into::into).collect())
    }
}
