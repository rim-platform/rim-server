//! Mapper between the SeaORM `capability_registry` model and the domain
//! `RegisteredCapability` struct in `rim_core`.

use crate::capability_registry;
use rim_core::capability::model::{CapabilitySourceKind, RegisteredCapability};

impl From<capability_registry::Model> for RegisteredCapability {
    fn from(m: capability_registry::Model) -> Self {
        let source_kind = match m.source_kind.as_str() {
            "builtin" => CapabilitySourceKind::Builtin,
            "plugin" => CapabilitySourceKind::Plugin,
            other => {
                tracing::warn!(
                    "unknown capability_registry.source_kind: {other}; defaulting to Plugin"
                );
                // Plugin is the more restrictive default — defaulting to
                // Builtin would risk auto-granting an unknown row to every
                // workspace member.
                CapabilitySourceKind::Plugin
            }
        };
        Self {
            capability_key: m.capability_key,
            source_kind,
            plugin_id: m.plugin_id,
            registered_at: m.registered_at.into(),
            retired_at: m.retired_at.map(Into::into),
            is_self_scoped: m.is_self_scoped,
        }
    }
}
