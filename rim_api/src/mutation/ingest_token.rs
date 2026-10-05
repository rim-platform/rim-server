use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct IngestTokenMutations;

#[Object]
impl IngestTokenMutations {
    // ── Alert ingest tokens (Track B2) ────────────────────────────────

    /// Mint a new alert-ingest token for a channel. Owner-only. The raw token is returned
    /// exactly once, in `rawToken` — it is never retrievable again after this call.
    async fn create_ingest_token(
        &self,
        ctx: &Context<'_>,
        input: CreateIngestTokenInput,
    ) -> Result<crate::types::CreateIngestTokenResultGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::ingest_token::service::IngestTokenService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let result = svc
            .create_token(rim_services::ingest_token::command::CreateIngestTokenCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                label: input.label,
                severity_default: input.severity_default.map(Into::into),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    /// Revoke an ingest token. Owner-only, permanent (no un-revoke).
    async fn revoke_ingest_token(&self, ctx: &Context<'_>, token_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::ingest_token::service::IngestTokenService>>(
            )?;
        let token_id = parse_uuid_id::<rim_core::ingest_token::model::IngestTokenId>(&token_id)?;

        svc.revoke_token(rim_services::ingest_token::command::RevokeIngestTokenCmd {
            actor: Actor::new(actor),
            token_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
struct CreateIngestTokenInput {
    server_id: ID,
    conversation_id: ID,
    label: String,
    severity_default: Option<crate::types::IncidentSeverityGql>,
}
