use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `message.flagged_at`: when the automatic text art check flagged the message; null while it is
/// not flagged. A change of the flag bumps the message's change-feed version, so the change
/// feed's update trigger is recreated with one more condition. `message_report.reporter_user_id`
/// becomes nullable: a null reporter is the automatic check, which files at most one report per
/// message.
#[derive(DeriveMigrationName)]
pub struct Migration;

const FEED_TRIGGER_SQL: &str = "CREATE TRIGGER message_change_feed_update BEFORE UPDATE ON message \
     FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
         OR NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext \
         OR NEW.flagged_at IS DISTINCT FROM OLD.flagged_at \
     ) EXECUTE FUNCTION message_change_feed()";

const PREVIOUS_FEED_TRIGGER_SQL: &str = "CREATE TRIGGER message_change_feed_update BEFORE UPDATE \
     ON message FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
         OR NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext \
     ) EXECUTE FUNCTION message_change_feed()";

const DROP_FEED_TRIGGER_SQL: &str = "DROP TRIGGER message_change_feed_update ON message";

const UP_SQL: &[&str] = &[
    "ALTER TABLE message ADD COLUMN flagged_at TIMESTAMPTZ NULL",
    DROP_FEED_TRIGGER_SQL,
    FEED_TRIGGER_SQL,
    "ALTER TABLE message_report ALTER COLUMN reporter_user_id DROP NOT NULL",
    "CREATE UNIQUE INDEX idx_message_report_message_automatic ON message_report (message_id) \
     WHERE reporter_user_id IS NULL",
];

const DOWN_SQL: &[&str] = &[
    "DROP INDEX IF EXISTS idx_message_report_message_automatic",
    "DELETE FROM message_report WHERE reporter_user_id IS NULL",
    "ALTER TABLE message_report ALTER COLUMN reporter_user_id SET NOT NULL",
    DROP_FEED_TRIGGER_SQL,
    PREVIOUS_FEED_TRIGGER_SQL,
    "ALTER TABLE message DROP COLUMN flagged_at",
];

async fn run(manager: &SchemaManager<'_>, statements: &[&str]) -> Result<(), DbErr> {
    let db = manager.get_connection();
    for sql in statements {
        db.execute_unprepared(sql).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, UP_SQL).await
    }

    /// Deletes the automatic reports, whose null reporter the restored constraint refuses.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, DOWN_SQL).await
    }
}
