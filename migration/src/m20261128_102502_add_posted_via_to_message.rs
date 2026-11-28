use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `message.posted_via`: how the message was posted when not by the person's own client, `agent`
/// for a message an agent's command call posted; null otherwise.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE message ADD COLUMN posted_via text NULL",
    "ALTER TABLE message ADD CONSTRAINT chk_message_posted_via CHECK (posted_via IN ('agent'))",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE message DROP CONSTRAINT chk_message_posted_via",
    "ALTER TABLE message DROP COLUMN posted_via",
];

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
