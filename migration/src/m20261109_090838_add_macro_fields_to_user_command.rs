use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `user_command.ai_callable`, `nl_description` and `intent_samples`: whether the AI agent may run
/// the macro, and how it is described to the agent. Every stored row gets the defaults (not
/// callable, no description, no samples), and the previous release's server never reads them.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE user_command ADD COLUMN ai_callable boolean NOT NULL DEFAULT false",
    "ALTER TABLE user_command ADD COLUMN nl_description text NOT NULL DEFAULT ''",
    "ALTER TABLE user_command ADD COLUMN intent_samples jsonb NOT NULL DEFAULT '[]'::jsonb",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE user_command DROP COLUMN intent_samples",
    "ALTER TABLE user_command DROP COLUMN nl_description",
    "ALTER TABLE user_command DROP COLUMN ai_callable",
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
