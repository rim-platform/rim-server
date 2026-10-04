//! P6-FU5 — Black Widow H2 high-latent finding closure.
//!
//! Adds `is_self_scoped BOOL NOT NULL DEFAULT false` to `capability_registry`.
//!
//! **Why**: `membership_holds(actor, None)` previously short-circuited to
//! `Ok(true)` unconditionally when `workspace_id` is absent. A future builtin
//! requiring a workspace-scoped cap (e.g. `chat.read`) dispatched without
//! workspace context would have silently bypassed the membership check.
//!
//! **What this column encodes**: a self-scoped capability (e.g.
//! `audit.read.self`) is one whose semantics do NOT require a workspace
//! context — the actor can exercise it purely on their own identity. All
//! other capabilities are workspace-scoped and MUST be called with a
//! `workspace_id`; `CapabilityService::check()` now enforces this via the
//! DB-stored flag rather than a hardcoded static set.
//!
//! **Backfill**: `audit.read.self` is the only builtin that is genuinely
//! self-scoped. Conservative default of `false` forces every other row to
//! require explicit workspace context — correct for all current builtins.
//!
//! **Idempotency**: `ALTER TABLE … ADD COLUMN IF NOT EXISTS` protects
//! re-entrant migration runs (e.g. dev environments where `migrate down`
//! was not run cleanly).

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add the column with a safe default so existing rows are backfilled
        // to `false` (workspace-scoped = requires workspace context). This is
        // the conservative choice: an unknown capability defaults to
        // "workspace-scoped" which forces callers to supply context, rather
        // than silently permitting context-free calls.
        manager
            .alter_table(
                Table::alter()
                    .table(CapabilityRegistry::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(CapabilityRegistry::IsSelfScoped)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Backfill: `audit.read.self` is the only builtin whose semantics are
        // self-scoped (reads the caller's own audit history; no workspace
        // membership is required or meaningful). All other builtins are
        // workspace-scoped and stay at `false`.
        //
        // This UPDATE is idempotent — re-running the migration is safe.
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE capability_registry \
                 SET    is_self_scoped = true \
                 WHERE  capability_key = 'audit.read.self'",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CapabilityRegistry::Table)
                    .drop_column(CapabilityRegistry::IsSelfScoped)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum CapabilityRegistry {
    Table,
    IsSelfScoped,
}
