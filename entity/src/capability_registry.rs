//! `SeaORM` entity for the capability registry table — Phase 1 §3.3 (closes
//! Black Widow C1). The table is the source of truth for which capability
//! keys exist; an unknown key is denied by default in
//! `CapabilityService::check`.
//!
//! Schema is intentionally minimal — see migration
//! `m20260424_120000_create_capability_registry` for the full column
//! contract and `rim_core::capability::repository::CapabilityRegistryRepository`
//! for the repository surface.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "capability_registry")]
pub struct Model {
    /// Canonical dotted capability key (e.g. `chat.read`,
    /// `plugin.youtrack.write`). Constrained at the application layer to
    /// 4..=128 chars per Widow N1 plugin-key validator.
    #[sea_orm(primary_key, auto_increment = false)]
    #[sea_orm(column_type = "String(StringLen::N(128))")]
    pub capability_key: String,

    /// Always `'builtin'`: every capability is seeded at startup.
    #[sea_orm(column_type = "String(StringLen::N(16))")]
    pub source_kind: String,

    pub registered_at: DateTimeWithTimeZone,

    /// Soft-retire timestamp. NULL = active. The runtime gate treats a
    /// retired row as unregistered, which closes the gate without touching
    /// audit history.
    pub retired_at: Option<DateTimeWithTimeZone>,

    /// P6-FU5 (Black Widow H2 closure). When `true` the capability is
    /// self-scoped: it does not require a workspace context and
    /// `CapabilityService::check()` accepts `workspace_id = None`.
    ///
    /// `DEFAULT false` at the DB level means rows default to
    /// workspace-scoped, which is the conservative/safe choice. Only `audit.read.self` (and any
    /// future cap explicitly declared self-scoped) is `true`.
    pub is_self_scoped: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
