use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::Actor;
use crate::ServiceResult;
use crate::audit::dto::{AuditLogEntryDto, AuditLogPage, ListAuditLogQuery};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AuditLogService: Send + Sync {
    /// Persist an audit log entry. Server-internal — no capability check.
    async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()>;

    /// Query audit log entries. Requires `audit.read` on the CALLER.
    /// `query.actor_user_id` is a row-filter predicate, not the auth principal.
    async fn list_entries(
        &self,
        caller: Actor,
        query: ListAuditLogQuery,
    ) -> ServiceResult<AuditLogPage>;
}
