use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::ingest_token::command::{
    CreateIngestTokenCmd, ListIngestTokensQuery, RevokeIngestTokenCmd,
};
use crate::ingest_token::dto::{CreateIngestTokenResultDto, IngestTokenDto};

/// Manages alert-ingestion webhook tokens (`POST /api/hooks/alerts/*` auth). See
/// `docs/alerts.md` for the end-to-end flow.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IngestTokenService: Send + Sync {
    /// Owner-only. Validates `conversation_id` is a channel in `server_id` and that
    /// `actor` (= the token's future `created_by`) is a member of it. Returns the raw
    /// token exactly once — it is never retrievable again.
    async fn create_token(
        &self,
        cmd: CreateIngestTokenCmd,
    ) -> ServiceResult<CreateIngestTokenResultDto>;

    /// Owner-only. Lists every token for the server, including revoked ones (the raw token
    /// is never included — only [`create_token`](Self::create_token) ever returns it).
    async fn list_tokens(&self, query: ListIngestTokensQuery)
    -> ServiceResult<Vec<IngestTokenDto>>;

    /// Owner-only. Permanent — no un-revoke.
    async fn revoke_token(&self, cmd: RevokeIngestTokenCmd) -> ServiceResult<()>;

    /// Verify a presented raw token: hash it, look up by hash, and reject if unknown or
    /// revoked. This is the webhook authentication path — called on every ingest request,
    /// not gated by any actor/session. Best-effort updates `last_used_at` on success (a
    /// failure to update must not fail authentication).
    ///
    /// # Errors
    /// Returns [`crate::ServiceError::Unauthenticated`] for an unknown or revoked token.
    async fn authenticate_token(&self, raw_token: &str) -> ServiceResult<IngestTokenDto>;
}
