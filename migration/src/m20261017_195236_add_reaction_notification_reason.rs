use sea_orm_migration::prelude::*;

/// Adds `reaction` to the `notification_reason` enum: a reaction on a message tells its author.
///
/// Postgres cannot drop an enum value, so `down` leaves it in place; a value nothing writes is
/// harmless to the previous release.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TYPE notification_reason ADD VALUE IF NOT EXISTS 'reaction'";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
