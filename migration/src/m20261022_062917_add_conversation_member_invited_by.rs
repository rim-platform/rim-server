use sea_orm_migration::prelude::*;

/// Adds `conversation_member.invited_by`: the member who added this one to the conversation,
/// whose principal the conversation's invite to them came from. `NULL` for a member the
/// conversation was created with, a member added before this column, and once the adder's
/// account is deleted.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE conversation_member ADD COLUMN invited_by uuid NULL \
     REFERENCES \"user\" (id) ON DELETE SET NULL ON UPDATE CASCADE";

const DOWN_SQL: &str = "ALTER TABLE conversation_member DROP COLUMN invited_by";

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
