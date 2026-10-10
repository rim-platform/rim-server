use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;

#[derive(Debug, Clone)]
pub struct AuditLogEntryRecord {
    pub id: uuid::Uuid,
    pub actor_user_id: uuid::Uuid,
    pub server_id: uuid::Uuid,
    pub command_id: String,
    pub initiator: String,
    pub args_redacted: serde_json::Value,
    pub sensitivity: String,
    pub dangerous: bool,
    pub confirmation_kind: String,
    pub typed_verb: Option<String>,
    pub outcome: String,
    pub error_kind: Option<String>,
    pub capability_check_outcome: Option<String>,
    pub confirmation_outcome: Option<String>,
    /// Set on child rows emitted for macro steps; references the parent macro
    /// audit row. `None` for top-level command audit entries.
    pub parent_audit_id: Option<uuid::Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct AuditLogListQuery {
    pub actor_user_id: Option<uuid::Uuid>,
    pub server_id: Option<uuid::Uuid>,
    pub command_id: Option<String>,
    pub typed_verb: Option<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    pub cursor: Option<(chrono::DateTime<chrono::Utc>, uuid::Uuid)>,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct AuditLogPageRecord {
    pub items: Vec<AuditLogEntryRecord>,
    pub next_cursor: Option<(chrono::DateTime<chrono::Utc>, uuid::Uuid)>,
    pub has_next_page: bool,
    pub has_prev_page: bool,
}

/// The audit rows older than a cutoff: how many there are and the oldest and newest
/// `created_at` among them (both `None` when there are none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditLogRange {
    pub count: u64,
    pub oldest: Option<chrono::DateTime<chrono::Utc>>,
    pub newest: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait AuditLogRepository: Send + Sync {
    async fn insert(&self, entry: AuditLogEntryRecord) -> RepoResult<()>;
    async fn list(&self, query: AuditLogListQuery) -> RepoResult<AuditLogPageRecord>;
    /// Delete rows with `created_at < cutoff`. Returns the number of rows deleted.
    async fn delete_older_than(&self, cutoff: chrono::DateTime<chrono::Utc>) -> RepoResult<u64>;
    /// Count the rows with `created_at < cutoff` and find their oldest and newest timestamps.
    async fn range_older_than(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<AuditLogRange>;
    /// Up to `limit` rows with `created_at < cutoff`, oldest first in `(created_at, id)` order,
    /// starting after the `after` key when given.
    async fn list_older_than(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
        after: Option<(chrono::DateTime<chrono::Utc>, uuid::Uuid)>,
        limit: u32,
    ) -> RepoResult<Vec<AuditLogEntryRecord>>;
    /// Delete up to `limit` of the oldest rows with `created_at < cutoff` whose
    /// `(created_at, id)` is at most `up_to`, in one statement. Returns the number deleted.
    async fn delete_older_than_batch(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
        up_to: (chrono::DateTime<chrono::Utc>, uuid::Uuid),
        limit: u32,
    ) -> RepoResult<u64>;
}
