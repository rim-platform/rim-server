use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `bot.member_joins_since`: from when an Owner or Moderator let a bot see members joining its
/// workspace, null while they don't. Null for every bot until someone turns it on, and the previous
/// release's server never reads it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &["ALTER TABLE bot ADD COLUMN member_joins_since timestamptz NULL"];

const DOWN_SQL: &[&str] = &["ALTER TABLE bot DROP COLUMN member_joins_since"];

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
