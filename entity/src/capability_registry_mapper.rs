//! Mapper between the SeaORM `capability_registry` model and the domain
//! `RegisteredCapability` struct in `rim_core`.

use crate::capability_registry;
use rim_core::capability::model::RegisteredCapability;

impl From<capability_registry::Model> for RegisteredCapability {
    fn from(m: capability_registry::Model) -> Self {
        Self {
            capability_key: m.capability_key,
            registered_at: m.registered_at.into(),
            retired_at: m.retired_at.map(Into::into),
            is_self_scoped: m.is_self_scoped,
        }
    }
}
