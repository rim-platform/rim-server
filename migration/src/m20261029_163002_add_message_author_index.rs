use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `idx_message_sent_by_sent_at`: serves one author's undeleted messages, newest first, for the
/// bulk delete of a person's messages. Building it blocks writes to `message` while it runs.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "CREATE INDEX idx_message_sent_by_sent_at ON message (sent_by, sent_at) \
     WHERE is_deleted = false",
];

const DOWN_SQL: &[&str] = &["DROP INDEX IF EXISTS idx_message_sent_by_sent_at"];

async fn run(manager: &SchemaManager<'_>, statements: &[&str]) -> Result<(), DbErr> {
    let db = manager.get_connection();
    for sql in statements {
        db.execute_unprepared(sql).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, UP_SQL).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, DOWN_SQL).await
    }
}
