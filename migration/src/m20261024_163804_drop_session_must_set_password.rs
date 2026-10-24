use sea_orm_migration::prelude::*;

/// Drops `session.must_set_password`. Every local account signs in with its password, so no
/// session is limited to setting one.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE session DROP COLUMN must_set_password")
            .await?;
        Ok(())
    }

    /// Adds the column back, false for every session.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE session ADD COLUMN must_set_password boolean NOT NULL DEFAULT false",
            )
            .await?;
        Ok(())
    }
}
