use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `bot.sees_member_joins`: whether an Owner or Moderator let a bot see members joining its
/// workspace. Off for every bot until someone turns it on, and the previous release's server never
/// reads it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] =
    &["ALTER TABLE bot ADD COLUMN sees_member_joins boolean NOT NULL DEFAULT false"];

const DOWN_SQL: &[&str] = &["ALTER TABLE bot DROP COLUMN sees_member_joins"];

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
