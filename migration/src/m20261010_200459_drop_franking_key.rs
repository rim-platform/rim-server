use sea_orm_migration::prelude::*;

/// Drops the `franking_key` table. The franking keys live in the key file; the table only fed
/// the one-time move of the keys into that file.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS franking_key")
            .await?;
        Ok(())
    }

    /// Recreates the table empty, as the migration that created it left it.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE TABLE franking_key (
                    id integer PRIMARY KEY,
                    secret bytea NOT NULL CHECK (octet_length(secret) = 32),
                    created_at timestamptz NOT NULL DEFAULT now()
                )"#,
            )
            .await?;
        Ok(())
    }
}
