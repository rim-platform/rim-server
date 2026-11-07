use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `home_layout`: one account's Home layout, the boxes it shows and their order, with when the
/// person made the edit (`edited_at`) and when the server stored it (`updated_at`).
///
/// The account purge deletes the row, since it keeps the user row as a tombstone; the cascade
/// covers a user row that is deleted outright. The previous release's server never reads the new
/// table.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[r#"CREATE TABLE home_layout (
    user_id uuid PRIMARY KEY REFERENCES "user" (id) ON DELETE CASCADE,
    layout jsonb NOT NULL,
    edited_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
)"#];

const DOWN_SQL: &[&str] = &["DROP TABLE home_layout"];

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
