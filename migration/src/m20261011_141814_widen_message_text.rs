use sea_orm_migration::prelude::*;

/// Widens `message.text` from `varchar(1024)` to `text`, so it holds the 10,000 characters a
/// message may have. The change feed's update trigger and the generated `search_tsv` column
/// depend on the column, so both are dropped and recreated around the type change; recreating
/// `search_tsv` rewrites every message row.
#[derive(DeriveMigrationName)]
pub struct Migration;

const FEED_TRIGGER_SQL: &str = "CREATE TRIGGER message_change_feed_update BEFORE UPDATE ON message \
     FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
         OR NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext \
     ) EXECUTE FUNCTION message_change_feed()";

const SEARCH_COLUMN_SQL: &str = "ALTER TABLE message ADD COLUMN search_tsv tsvector \
     GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED";

const SEARCH_INDEX_SQL: &str =
    "CREATE INDEX idx_message_search_tsv ON message USING GIN (search_tsv)";

const DETACH_SQL: &[&str] = &[
    "DROP TRIGGER message_change_feed_update ON message",
    "DROP INDEX IF EXISTS idx_message_search_tsv",
    "ALTER TABLE message DROP COLUMN search_tsv",
];

const REATTACH_SQL: &[&str] = &[SEARCH_COLUMN_SQL, SEARCH_INDEX_SQL, FEED_TRIGGER_SQL];

async fn run(manager: &SchemaManager<'_>, retype: &str) -> Result<(), DbErr> {
    let db = manager.get_connection();
    for sql in DETACH_SQL {
        db.execute_unprepared(sql).await?;
    }
    db.execute_unprepared(retype).await?;
    for sql in REATTACH_SQL {
        db.execute_unprepared(sql).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, "ALTER TABLE message ALTER COLUMN text TYPE text").await
    }

    /// Narrows the column back; text past 1024 characters is cut off.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(
            manager,
            "ALTER TABLE message ALTER COLUMN text TYPE varchar(1024) USING left(text, 1024)",
        )
        .await
    }
}
