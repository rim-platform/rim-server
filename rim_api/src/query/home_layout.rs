use std::sync::Arc;

use async_graphql::*;
use rim_services::Actor;
use rim_services::home_layout::service::HomeLayoutService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::HomeLayout;

#[derive(Default)]
pub(crate) struct HomeLayoutQueries;

#[Object]
impl HomeLayoutQueries {
    /// The signed-in person's Home layout, or null when none is stored and the client shows its
    /// default Home. `FORBIDDEN` for a person from another server, whose layout lives at their
    /// own server.
    async fn home_layout(&self, ctx: &Context<'_>) -> Result<Option<HomeLayout>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<Arc<dyn HomeLayoutService>>()?;
        svc.home_layout(Actor::new(actor))
            .await
            .map(|layout| layout.map(Into::into))
            .map_err(map_service_error)
    }
}
