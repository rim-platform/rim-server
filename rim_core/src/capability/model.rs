use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Source of a registered capability — controls who is treated as
/// "holding" it inside [`crate::capability::repository::CapabilityRegistryRepository`].
///
/// Phase 1 policy (per design doc §3.3):
/// - `Builtin` — every authenticated workspace member holds this.
/// - `Plugin` — held only by actors whom the plugin's grant store explicitly
///   accepts. No grant store ships in Phase 1, so `Plugin` keys deny by
///   default until Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySourceKind {
    Builtin,
    Plugin,
}

impl CapabilitySourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CapabilitySourceKind::Builtin => "builtin",
            CapabilitySourceKind::Plugin => "plugin",
        }
    }
}

/// A row from the `capability_registry` table — the source-of-truth for
/// "does this capability key exist at all?". Unknown keys deny by default
/// in [`crate::capability::repository::CapabilityRegistryRepository::is_registered`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCapability {
    pub capability_key: String,
    pub source_kind: CapabilitySourceKind,
    /// Populated only when `source_kind = Plugin`.
    pub plugin_id: Option<Uuid>,
    pub registered_at: DateTime<Utc>,
    /// Soft-retire timestamp. `None` ⇒ active. The `is_registered` gate
    /// filters on `retired_at IS NULL` so retiring a plugin's caps cleanly
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
