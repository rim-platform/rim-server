//! Phase 1 §3.3 — capability_registry table.
//!
//! Closes Black Widow C1 ("real capability gate from Phase 1, no no-op-allow
//! stub"). The table is the source of truth for which capability keys exist
//! at all; an unknown key denies by default in `CapabilityService::check`.
//!
//! Schema:
//! - `capability_key` — primary key, the canonical dotted identifier
//!   (`chat.read`, `plugin.<plugin_id>.<verb>`, ...). Length cap matches
//!   N1 plugin-key validator (4..=128).
//! - `source_kind` — `'builtin'` (seeded synchronously at startup) or
//!   `'plugin'` (upserted at plugin install, retired at uninstall).
//! - `plugin_id` — nullable FK target; populated only when `source_kind =
//!   'plugin'`. Phase 1 leaves the FK off because plugin entities don't
//!   ship until Phase 5; constraint added when plugin_install table lands.
//! - `registered_at` — initial registration timestamp.
//! - `retired_at` — soft-retire on uninstall (Phase 5). NULL = active.
//!
//! Operationally, `is_registered(key)` filters on `retired_at IS NULL`, so
//! retiring a plugin's caps cleanly closes the gate for subsequent calls
//! without needing cascade deletes through audit history.
//!
//! Schema-layer integrity (Gate 4 M6 + Fury O-5):
//! - `CHECK (source_kind IN ('builtin', 'plugin'))` — strict-deny gate
//!   misclassification protection.
//! - `CHECK ((source_kind='builtin' AND plugin_id IS NULL) OR
//!           (source_kind='plugin'  AND plugin_id IS NOT NULL))` — prevents
//!   a builtin row from being collected by a plugin uninstall sweep, and
//!   prevents an orphaned plugin row that no sweep can ever find.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CapabilityRegistry::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CapabilityRegistry::CapabilityKey)
                            .string_len(128)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CapabilityRegistry::SourceKind)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(CapabilityRegistry::PluginId).uuid().null())
                    .col(
                        ColumnDef::new(CapabilityRegistry::RegisteredAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(CapabilityRegistry::RetiredAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // INDEX: (plugin_id) — supports retire-by-plugin sweeps when a
        // plugin uninstalls (Phase 5). Cheap on small registries; keeps the
        // sweep query off a sequential scan once the plugin ecosystem grows.
        manager
            .create_index(
                Index::create()
                    .name("idx_capability_registry_plugin_id")
                    .table(CapabilityRegistry::Table)
                    .col(CapabilityRegistry::PluginId)
                    .to_owned(),
            )
            .await?;

        // INDEX: (source_kind, retired_at) — reserved for Phase 5 inventory
        // queries (`SELECT WHERE source_kind = 'plugin' AND retired_at IS
        // NULL`). The hot-path `is_registered(key)` query is covered by the
        // PK index alone — `WHERE capability_key = $1 AND retired_at IS
        // NULL` only needs the PK lookup; the secondary index here is for
        // the future bulk-list path, not the per-request gate.
        manager
            .create_index(
                Index::create()
                    .name("idx_capability_registry_source_kind_retired_at")
                    .table(CapabilityRegistry::Table)
                    .col(CapabilityRegistry::SourceKind)
                    .col(CapabilityRegistry::RetiredAt)
                    .to_owned(),
            )
            .await?;

        // CHECK constraints — Black Widow Gate 4 M6 + Fury O-5.
        //
        // Defense-in-depth at the storage layer. Without these, a buggy
        // migration / direct SQL / future repo refactor could persist:
        //   - `source_kind = 'BUILTIN'` (case mismatch) → strict-deny gate
        //     silently misclassifies every row
        //   - `source_kind = 'builtin'` with `plugin_id IS NOT NULL` →
        //     `retire_for_plugin` sweep accidentally retires builtin caps
        //   - `source_kind = 'plugin'` with `plugin_id IS NULL` →
        //     orphaned plugin row that no uninstall sweep can ever find
        //
        // Postgres-only syntax; that's intentional — RIM ships on PG.
        let stmts = [
            "ALTER TABLE capability_registry ADD CONSTRAINT capability_registry_source_kind_check \
             CHECK (source_kind IN ('builtin', 'plugin'))",
            "ALTER TABLE capability_registry ADD CONSTRAINT capability_registry_plugin_id_consistency_check \
             CHECK ((source_kind = 'builtin' AND plugin_id IS NULL) OR \
                    (source_kind = 'plugin'  AND plugin_id IS NOT NULL))",
        ];
        for sql in stmts {
            manager.get_connection().execute_unprepared(sql).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CapabilityRegistry::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum CapabilityRegistry {
    Table,
    CapabilityKey,
    SourceKind,
    PluginId,
    RegisteredAt,
    RetiredAt,
}
