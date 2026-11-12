use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `incoming_webhook.template`: a webhook's own format, null for one that follows its preset. Null
/// for every webhook until someone saves a format, and the previous release's server never reads
/// it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &["ALTER TABLE incoming_webhook ADD COLUMN template jsonb NULL"];

const DOWN_SQL: &[&str] = &["ALTER TABLE incoming_webhook DROP COLUMN template"];

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
