use sea_orm_migration::prelude::*;

/// Stores the latest authenticated activity independently of expiring sessions.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE TABLE user_activity (
                user_id uuid PRIMARY KEY REFERENCES "user"(id) ON DELETE CASCADE,
                last_authenticated_at timestamptz NOT NULL
            )"#,
            )
            .await?;
        manager.get_connection().execute_unprepared(
            "CREATE INDEX idx_user_activity_last_authenticated ON user_activity (last_authenticated_at)",
        ).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE user_activity")
            .await?;
        Ok(())
    }
}
