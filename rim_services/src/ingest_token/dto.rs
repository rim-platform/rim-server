use chrono::{DateTime, Utc};

use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::IncidentSeverity;
use rim_core::ingest_token::model::{IngestToken, IngestTokenId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IngestTokenDto {
    pub id: IngestTokenId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub created_by: UserId,
    pub label: String,
    pub severity_default: Option<IncidentSeverity>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<IngestToken> for IngestTokenDto {
    fn from(t: IngestToken) -> Self {
        Self {
            id: t.id,
            server_id: t.server_id,
            conversation_id: t.conversation_id,
            created_by: t.created_by,
            label: t.label,
            severity_default: t.severity_default,
            created_at: t.created_at,
            revoked_at: t.revoked_at,
            last_used_at: t.last_used_at,
        }
    }
}

/// Result of [`crate::ingest_token::service::IngestTokenService::create_token`]. `raw_token` is
/// surfaced exactly once — like invite tokens, only the hash is ever persisted or retrievable
/// again after this call returns.
#[derive(Debug, Clone)]
pub struct CreateIngestTokenResultDto {
    pub token: IngestTokenDto,
    pub raw_token: String,
}
