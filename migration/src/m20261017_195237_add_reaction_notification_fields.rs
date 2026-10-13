use sea_orm_migration::prelude::*;

/// Adds what a reaction notification needs:
/// - `user_notification.reaction_emoji` and `user_notification.reaction_by` (the reactor), set
///   exactly when `reason` is `reaction`;
/// - `user_notification_settings.reactions`, whether reactions on the user's messages notify
///   them (default true).
///
/// Nullable columns and a column with a default: the previous release's server keeps working.
/// The check compares `reason` as text because the enum value added by the previous migration
/// cannot be used as a literal in the same transaction.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE user_notification ADD COLUMN reaction_emoji text, \
     ADD COLUMN reaction_by uuid",
    "ALTER TABLE user_notification ADD CONSTRAINT chk_user_notification_reaction_fields \
     CHECK ((reason::text = 'reaction') = (reaction_emoji IS NOT NULL AND reaction_by IS NOT NULL))",
    "ALTER TABLE user_notification_settings ADD COLUMN reactions boolean NOT NULL DEFAULT true",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE user_notification_settings DROP COLUMN reactions",
    "DELETE FROM user_notification WHERE reason::text = 'reaction'",
    "ALTER TABLE user_notification DROP CONSTRAINT chk_user_notification_reaction_fields",
    "ALTER TABLE user_notification DROP COLUMN reaction_by, DROP COLUMN reaction_emoji",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in UP_SQL {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in DOWN_SQL {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
