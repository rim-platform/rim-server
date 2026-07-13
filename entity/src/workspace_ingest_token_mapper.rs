use crate::workspace_ingest_token;
use rim_core::ingest_token::model::IngestToken;

impl From<workspace_ingest_token::Model> for IngestToken {
    fn from(m: workspace_ingest_token::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            conversation_id: m.conversation_id.into(),
            created_by: m.created_by.into(),
            label: m.label,
            token_hash: m.token_hash,
            severity_default: m.severity_default.and_then(|s| s.parse().ok()),
            created_at: m.created_at.into(),
            revoked_at: m.revoked_at.map(Into::into),
            last_used_at: m.last_used_at.map(Into::into),
        }
    }
}
