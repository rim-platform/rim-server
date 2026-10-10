use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::capability::model::RegisteredCapability;
use crate::repository::error::RepoResult;

/// Persistence surface for the `capability_registry` table — Phase 1 §3.3
/// (closes Black Widow C1). The runtime gate calls `get` on every capability
/// the actor's required capability list mentions; an unknown or retired key
/// denies by default.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait CapabilityRegistryRepository: Send + Sync {
    /// Seeds a builtin capability. Insert-only: write the row when missing, **leave
    /// existing rows untouched** (including their `retired_at` value), so a
    /// deploy cannot un-retire an operator-disabled builtin: an operator who
    /// soft-retires `audit.read` expects that policy to survive every redeploy.
    ///
    /// `is_self_scoped` — P6-FU5: stored alongside the key so
    /// `CapabilityService::check()` can enforce workspace-context requirements
    /// from the DB rather than a hardcoded static set, using the value from
    /// [`BuiltinCapabilitySpec`].
    async fn upsert_if_missing(&self, capability_key: &str, is_self_scoped: bool)
    -> RepoResult<()>;

    /// Returns the full registry row, retired or not. The runtime gate treats a
    /// missing row or one with `retired_at` set as unregistered.
    async fn get(&self, capability_key: &str) -> RepoResult<Option<RegisteredCapability>>;
}
