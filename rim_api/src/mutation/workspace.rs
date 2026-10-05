use async_graphql::*;
use rim_services::Actor;
use rim_services::server::command::CreateServerCmd;
use rim_services::server::service::ServerService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::Server;

#[derive(Default)]
pub(crate) struct WorkspaceMutations;

#[Object]
impl WorkspaceMutations {
    async fn create_server(&self, ctx: &Context<'_>, input: CreateServerInput) -> Result<Server> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let server = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor),
                name: input.name,
            })
            .await
            .map_err(map_service_error)?;

        Ok(server.into())
    }
}

#[derive(InputObject)]
pub struct CreateServerInput {
    pub name: String,
}
