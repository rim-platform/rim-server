use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `user.profile_locked_until`: until when the instance's operators locked the account's
/// display name, picture, status and bio; null while no lock is set.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"ALTER TABLE "user" ADD COLUMN profile_locked_until timestamptz NULL"#;

const DOWN_SQL: &str = r#"ALTER TABLE "user" DROP COLUMN profile_locked_until"#;

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
