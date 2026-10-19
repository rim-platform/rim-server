use sea_orm_migration::prelude::*;

/// Adds a unique index on the lowercased `user.username`, so two accounts cannot hold names that
/// differ only in case. The index on the raw column stays.
///
/// Fails on a database that already holds two such names; one of them is renamed first.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str =
    r#"CREATE UNIQUE INDEX "idx_user_username_lower_unique" ON "user" (lower(username))"#;

const DOWN_SQL: &str = r#"DROP INDEX IF EXISTS "idx_user_username_lower_unique""#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
