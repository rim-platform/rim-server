//! Creates the `capability_registry` table: the source of truth for which
//! capability keys exist at all. An unknown key denies by default in
//! `CapabilityService::check`.
//!
//! Schema:
//! - `capability_key` — primary key, the canonical dotted identifier
//!   (`chat.read`, `plugin.<plugin_id>.<verb>`, ...), at most 128 characters.
//! - `source_kind` — `'builtin'` (seeded synchronously at startup) or
//!   `'plugin'` (upserted at plugin install, retired at uninstall).
//! - `plugin_id` — nullable, set only when `source_kind = 'plugin'`. It has
//!   no FK because there is no plugin table to reference.
//!   `m20261010_205037_drop_redundant_indexes` drops the plugin column.
//! - `registered_at` — initial registration timestamp.
//! - `retired_at` — soft-retire timestamp. NULL = active.
//!
//! Operationally, `is_registered(key)` filters on `retired_at IS NULL`, so
//! retiring a plugin's caps cleanly closes the gate for subsequent calls
//! without needing cascade deletes through audit history.
//!
//! Schema-layer integrity:
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

        // INDEX: (plugin_id) — supports retire-by-plugin sweeps.
        manager
            .create_index(
                Index::create()
                    .name("idx_capability_registry_plugin_id")
                    .table(CapabilityRegistry::Table)
                    .col(CapabilityRegistry::PluginId)
                    .to_owned(),
            )
            .await?;

        // INDEX: (source_kind, retired_at) — for inventory queries
        // (`SELECT WHERE source_kind = 'plugin' AND retired_at IS NULL`).
        // The per-request `is_registered(key)` query only needs the PK.
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

        // CHECK constraints.
        //
        // Defense-in-depth at the storage layer. Without these, a buggy
        // migration / direct SQL / repository change could persist:
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
