use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::IncidentSeverity;
use rim_core::ingest_token::model::IngestTokenId;
use rim_core::server::model::ServerId;

use crate::Actor;

/// Only the server **owner** may create an ingest token — enforced via `ensure_server_owner`
/// in the service impl. `conversation_id` must be a channel in `server_id`.
#[derive(Debug, Clone)]
pub struct CreateIngestTokenCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub label: String,
    pub severity_default: Option<IncidentSeverity>,
}

/// Owner-only: list every ingest token (including revoked) for a server.
#[derive(Debug, Clone)]
pub struct ListIngestTokensQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Owner-only: revoke a token by id. Revocation is permanent (no un-revoke).
#[derive(Debug, Clone)]
pub struct RevokeIngestTokenCmd {
    pub actor: Actor,
    pub token_id: IngestTokenId,
}
