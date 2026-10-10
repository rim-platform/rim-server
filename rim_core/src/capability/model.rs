use chrono::{DateTime, Utc};

/// A row from the `capability_registry` table — the source-of-truth for
/// "does this capability key exist at all?". Unknown keys deny by default
/// in the runtime gate, which reads rows through
/// [`crate::capability::repository::CapabilityRegistryRepository::get`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCapability {
    pub capability_key: String,
    pub registered_at: DateTime<Utc>,
    /// Soft-retire timestamp. `None` ⇒ active. The runtime gate treats a
    /// row with `retired_at` set as unregistered so retiring a capability cleanly
    /// closes the gate without touching audit history.
    pub retired_at: Option<DateTime<Utc>>,
    /// When `true` the capability is self-scoped: it does not require a
    /// workspace context and `CapabilityService::check()` accepts
    /// `workspace_id = None` for it.
    ///
    /// When `false` (the conservative default) the capability is
    /// workspace-scoped: `check()` rejects `workspace_id = None` with
    /// `PermissionDenied` — the membership check would be meaningless without
    /// a workspace and passing `None` was previously a silent security bypass
    /// (P6-FU5, Black Widow H2).
    pub is_self_scoped: bool,
}
