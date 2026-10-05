use async_graphql::*;
use rim_services::Actor;
use rim_services::server::command::{GetServerQuery, ListMyServersQuery};
use rim_services::server::service::ServerService;
use rim_services::server_member::command::ListServerMembersQuery;
use rim_services::server_member::service::ServerMemberService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::{Server, ServerMember};

#[derive(Default)]
pub(crate) struct WorkspaceQueries;

#[Object]
impl WorkspaceQueries {
    async fn my_servers(&self, ctx: &Context<'_>) -> Result<Vec<Server>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;

        let servers = svc
            .list_my_servers(ListMyServersQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(servers.into_iter().map(Into::into).collect())
    }

    async fn server(&self, ctx: &Context<'_>, server_id: ID) -> Result<Server> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let dto = svc
            .get_server(GetServerQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dto.into())
    }
}

#[derive(Default)]
pub(crate) struct WorkspaceMemberQueries;

#[Object]
impl WorkspaceMemberQueries {
    async fn server_members(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<ServerMember>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerMemberService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let members = svc
            .list_members(ListServerMembersQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(members.into_iter().map(Into::into).collect())
    }
}
