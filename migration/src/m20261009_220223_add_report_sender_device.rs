use sea_orm_migration::prelude::*;

/// `message_report.franked_sender_device_id`: the sender's client device id of a verified
/// report of an encrypted message, as the receipt covered it.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE message_report ADD COLUMN franked_sender_device_id uuid NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE message_report DROP COLUMN franked_sender_device_id")
            .await?;
        Ok(())
    }
}
