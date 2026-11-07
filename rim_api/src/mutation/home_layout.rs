use std::sync::Arc;

use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::home_layout::request::SaveHomeLayoutCmd;
use rim_services::home_layout::service::HomeLayoutService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::HomeLayoutSaved;

#[derive(Default)]
pub(crate) struct HomeLayoutMutations;

#[Object]
impl HomeLayoutMutations {
    /// Save the signed-in person's Home layout, made at `editedAt` on their device. The stored
    /// layout is kept instead when its `editedAt` is later, an `editedAt` later than the
    /// server's clock counting as the clock; `kept` says which. `VALIDATION` names the check a
    /// layout fails; `FORBIDDEN` for a person from another server.
    async fn save_home_layout(
        &self,
        ctx: &Context<'_>,
        layout: Json<serde_json::Value>,
        edited_at: DateTime<Utc>,
    ) -> Result<HomeLayoutSaved> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<Arc<dyn HomeLayoutService>>()?;
        svc.save_home_layout(SaveHomeLayoutCmd {
            actor: Actor::new(actor),
            layout: layout.0,
            edited_at,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Delete the signed-in person's Home layout, so every device shows the default. Answers
    /// whether one was stored. `FORBIDDEN` for a person from another server.
    async fn reset_home_layout(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<Arc<dyn HomeLayoutService>>()?;
        svc.reset_home_layout(Actor::new(actor))
            .await
            .map_err(map_service_error)
    }
}
