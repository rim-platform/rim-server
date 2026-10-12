use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::account_deletion::dto::{
    AccountDeletionScheduleDto, PurgeRunDto, RequestAccountDeletionCmd,
};
use crate::{Actor, ServiceResult};

/// Self-service account deletion. A request starts a grace period during which the account
/// keeps working and may cancel; once it ends, the purge removes what the account owns and
/// leaves a tombstone row that kept encrypted messages and other shared rows still name.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AccountDeletionService: Send + Sync {
    /// Schedule the caller's account for deletion after the grace period and end its other
    /// sessions.
    ///
    /// # Errors
    /// `Forbidden` for a person from another server (they delete their account at their relay);
    /// `Validation` when `confirm_username` is not the caller's username or the caller is the
    /// only owner of a workspace with other members; `Conflict` while a deletion is already
    /// pending.
    async fn request_deletion(
        &self,
        cmd: RequestAccountDeletionCmd,
    ) -> ServiceResult<AccountDeletionScheduleDto>;

    /// Cancel the caller's pending deletion; whether one was pending.
    async fn cancel_deletion(&self, actor: Actor) -> ServiceResult<bool>;

    /// Purge every account whose grace period ended by `now`, each in its own transaction. An
    /// account whose purge fails is reported and skipped; the others go on. With `dry_run`
    /// every transaction rolls back and nothing outside the database is touched.
    async fn purge_due(&self, now: DateTime<Utc>, dry_run: bool) -> ServiceResult<PurgeRunDto>;
}
