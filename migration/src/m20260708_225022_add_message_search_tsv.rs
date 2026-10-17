use sea_orm_migration::prelude::*;

/// Adds `message.search_tsv`, a `GENERATED ALWAYS ... STORED` `tsvector` column backing Postgres
/// full-text search, plus a GIN index on it.
///
/// The column needs no trigger: `to_tsvector('english', …)` with a constant config literal is
/// IMMUTABLE, so a `GENERATED ALWAYS AS … STORED` column is valid and stays in sync with
/// `message.text`. `sea-query` cannot express generated columns, so `up` and `down` use raw
/// `execute_unprepared`.
///
/// Adding a `STORED` generated column rewrites the whole `message` table and holds an
/// `ACCESS EXCLUSIVE` lock while existing rows are backfilled, so no separate data migration is
/// needed. The index is not built with `CREATE INDEX CONCURRENTLY`: migrations run inside a
/// transaction, and that statement cannot run inside one.
///
/// The text-search configuration is the hardcoded `'english'`.
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
