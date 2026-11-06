use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `idx_message_mention_user_created`: serves one user's mentions in the order they were made, for
/// a bot polling for its mentions. Building it blocks writes to `message_mention` while it runs.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "CREATE INDEX idx_message_mention_user_created ON message_mention \
     (mentioned_user_id, created_at, message_id)",
];

const DOWN_SQL: &[&str] = &["DROP INDEX IF EXISTS idx_message_mention_user_created"];

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
