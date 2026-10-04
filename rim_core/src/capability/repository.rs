use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::capability::model::{Capability, CapabilitySourceKind, RegisteredCapability};
use crate::repository::error::RepoResult;

/// Read-only access to the human-facing capability catalogue surfaced in
/// Settings → Capabilities. NOT consulted by the runtime gate — see
/// [`CapabilityRegistryRepository`] for that.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait CapabilityRepository: Send + Sync {
    async fn list_capabilities(&self) -> RepoResult<Vec<Capability>>;
    async fn get_capability(&self, id: &str) -> RepoResult<Option<Capability>>;
}

/// Persistence surface for the `capability_registry` table — Phase 1 §3.3
/// (closes Black Widow C1). The runtime gate calls `is_registered` on every
/// capability the actor's required capability list mentions; an unknown key
/// denies by default.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait CapabilityRegistryRepository: Send + Sync {
    /// Returns `Ok(true)` when the key is present in the registry AND not
    /// retired. Returns `Ok(false)` otherwise — never silently allows.
    async fn is_registered(&self, capability_key: &str) -> RepoResult<bool>;

    /// Insert-only seed semantics: write the row when missing, **leave
    /// existing rows untouched** (including their `retired_at` value).
    ///
    /// This is the boot-path entrypoint for builtin-cap seeding. It is
    /// deliberately distinct from [`Self::upsert`] so a deploy cannot
    /// silently un-retire an operator-disabled builtin (Black Widow Gate 4
    /// finding M3): an operator who soft-retires `audit.read` company-wide
    /// expects that policy to survive every redeploy. The boot path calls
    /// this method; only plugin-install / explicit re-classify paths call
    /// the active-write `upsert`.
    ///
    /// `is_self_scoped` — P6-FU5: stored alongside the key so
    /// `CapabilityService::check()` can enforce workspace-context requirements
    /// from the DB rather than a hardcoded static set. Builtin caps use the
    /// value from [`BuiltinCapabilitySpec`]; plugin caps should default to
    /// `false` (workspace-scoped) unless the plugin explicitly declares
    /// otherwise.
    async fn upsert_if_missing(
        &self,
        capability_key: &str,
        source_kind: CapabilitySourceKind,
        plugin_id: Option<uuid::Uuid>,
        is_self_scoped: bool,
    ) -> RepoResult<()>;

    /// Active upsert: insert when missing; on existing row, refresh
    /// `source_kind` / `plugin_id` / `is_self_scoped` AND clear `retired_at`.
    ///
    /// Reserved for **plugin lifecycle** (re-install lifts a prior
    /// soft-retire cleanly) and explicit operator re-classify flows. The
    /// boot path MUST NOT call this — see [`Self::upsert_if_missing`].
    async fn upsert(
        &self,
        capability_key: &str,
        source_kind: CapabilitySourceKind,
        plugin_id: Option<uuid::Uuid>,
        is_self_scoped: bool,
    ) -> RepoResult<()>;

    /// Marks every still-active row matching the predicate as retired.
    /// Used at plugin uninstall (Phase 5) — kept on the trait now so the
    /// service surface is stable across phases.
    async fn retire_for_plugin(&self, plugin_id: uuid::Uuid) -> RepoResult<()>;

    /// Returns the full registry row for diagnostics / `audit_inventory`
    /// emission. Hot-path callers should prefer `is_registered`.
    async fn get(&self, capability_key: &str) -> RepoResult<Option<RegisteredCapability>>;
}
