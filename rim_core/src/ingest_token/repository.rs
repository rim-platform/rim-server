use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ingest_token::model::{IngestToken, IngestTokenId, NewIngestToken};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IngestTokenRepository: Send + Sync {
    async fn create(&self, new: NewIngestToken) -> RepoResult<IngestToken>;

    async fn find_by_id(&self, id: IngestTokenId) -> RepoResult<Option<IngestToken>>;

    /// O(1) hash lookup — the webhook authentication path. Returns the token record
    /// regardless of revocation status; the caller (service layer) checks `revoked_at`.
    async fn find_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<IngestToken>>;

    /// List all tokens (including revoked ones) for a server, for owner-facing management UI.
    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<IngestToken>>;

    async fn revoke(&self, id: IngestTokenId) -> RepoResult<()>;

    /// Best-effort last-used timestamp update. Failure here should never fail the ingest
    /// request itself — callers may ignore the result.
    async fn touch_last_used(&self, id: IngestTokenId, at: DateTime<Utc>) -> RepoResult<()>;
}
