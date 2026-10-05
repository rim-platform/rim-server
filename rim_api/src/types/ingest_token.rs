use async_graphql::SimpleObject;

use super::IncidentSeverityGql;

/// GraphQL projection of an alert-ingest token (Track B2). Never exposes `token_hash` — the raw
/// token is only ever returned once, at creation, via [`CreateIngestTokenResultGql::raw_token`].
#[derive(SimpleObject, Clone, Debug)]
pub struct IngestTokenGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub created_by: async_graphql::ID,
    pub label: String,
    pub severity_default: Option<IncidentSeverityGql>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::ingest_token::dto::IngestTokenDto> for IngestTokenGql {
    fn from(value: rim_services::ingest_token::dto::IngestTokenDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            created_by: async_graphql::ID(value.created_by.to_string()),
            label: value.label,
            severity_default: value.severity_default.map(Into::into),
            created_at: value.created_at,
            revoked_at: value.revoked_at,
            last_used_at: value.last_used_at,
        }
    }
}

/// Result of `createIngestToken` — `raw_token` is surfaced exactly once and is never
/// retrievable again (mirrors invite-token creation).
#[derive(SimpleObject, Clone, Debug)]
pub struct CreateIngestTokenResultGql {
    pub token: IngestTokenGql,
    pub raw_token: String,
}

impl From<rim_services::ingest_token::dto::CreateIngestTokenResultDto>
    for CreateIngestTokenResultGql
{
    fn from(value: rim_services::ingest_token::dto::CreateIngestTokenResultDto) -> Self {
        Self {
            token: value.token.into(),
            raw_token: value.raw_token,
        }
    }
}
