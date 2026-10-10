use sea_orm_migration::prelude::*;

/// Drops the foreign key from `message.franking_key_id` to `franking_key`: franking keys live in
/// the server's key file, whose ids the table does not hold.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE message DROP CONSTRAINT IF EXISTS message_franking_key_id_fkey",
            )
            .await?;
        Ok(())
    }

    /// Restores the constraint without checking existing rows, which may name keys the table
    /// never held.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE message ADD CONSTRAINT message_franking_key_id_fkey \
                 FOREIGN KEY (franking_key_id) REFERENCES franking_key (id) NOT VALID",
            )
            .await?;
        Ok(())
    }
}
