use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct IngestTokenQueries;

#[Object]
impl IngestTokenQueries {
    /// List every alert-ingest token for a server (including revoked). Owner-only.
    async fn ingest_tokens(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<crate::types::IngestTokenGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::ingest_token::service::IngestTokenService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let tokens = svc
            .list_tokens(rim_services::ingest_token::command::ListIngestTokensQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(tokens.into_iter().map(Into::into).collect())
    }
}
