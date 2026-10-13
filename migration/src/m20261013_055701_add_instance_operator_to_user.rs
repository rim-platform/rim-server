use sea_orm_migration::prelude::*;

/// Adds `user.instance_operator`: whether the account may run the instance's moderation queue
/// from the client (abuse reports, principal blocks, IP blocks). False for every existing
/// account; the `rim_server operator` command grants and revokes it.
///
/// A column with a default: the previous release's server keeps working on it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str =
    r#"ALTER TABLE "user" ADD COLUMN instance_operator boolean NOT NULL DEFAULT false"#;

const DOWN_SQL: &str = r#"ALTER TABLE "user" DROP COLUMN instance_operator"#;

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
