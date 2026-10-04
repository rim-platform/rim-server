use sea_orm_migration::prelude::*;

/// Adds `message.search_tsv`, a `GENERATED ALWAYS ... STORED` `tsvector` column backing Postgres
/// full-text search, plus a GIN index on it.
///
/// **Why a STORED generated column, not a trigger:** `to_tsvector('english', …)` with a constant
/// config literal is IMMUTABLE, so a `GENERATED ALWAYS AS … STORED` column is valid and stays in
/// sync automatically with zero trigger maintenance. `sea-query` can't express generated columns,
/// so `up`/`down` use raw `execute_unprepared` (consistent with how this codebase adds CHECK
/// constraints/indexes elsewhere, e.g. `m20260427_000001_audit_outcome_widen_check`).
///
/// **Ops / rollback note:** adding a `STORED` generated column rewrites the entire `message` table
/// and takes an `ACCESS EXCLUSIVE` lock for the duration of the backfill. At RIM's current
/// self-hosted scale this is acceptable during a deploy window. For a large existing table, the
/// low-lock alternative is: add a plain nullable `tsvector` column, backfill in batches, add a
/// `BEFORE INSERT/UPDATE` trigger, then `CREATE INDEX CONCURRENTLY`. Do **not** attempt the
/// concurrent variant inside a sea-orm migration — migrations run inside a transaction, and
/// `CREATE INDEX CONCURRENTLY` cannot run inside one. This migration uses the simpler
/// generated-column form; the alternative is noted here for a future scale-driven revisit.
/// Forward-only: the column backfills existing rows automatically as part of `up`; no separate
/// data migration is needed.
///
/// **Config note:** `'english'` is hardcoded as the text-search configuration. If multilingual
/// content matters later, that's a follow-up (per-workspace config, or a `simple` fallback).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE message \
                 ADD COLUMN search_tsv tsvector \
                 GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED",
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_message_search_tsv ON message USING GIN (search_tsv)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_message_search_tsv")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE message DROP COLUMN IF EXISTS search_tsv")
            .await?;
        Ok(())
    }
}
