use sea_orm_migration::prelude::*;

/// Adds how each account was created to `user`: `created_by_invite_id`, the instance or
/// workspace invite it signed up with, and `invited_by_user_id`, that invite's creator.
///
/// Nullable, no backfill: accounts created before, by an open or proof-of-work signup, or with
/// an operator's invite have none. `created_by_invite_id` names a row of either invite table,
/// so it carries no foreign key; `invited_by_user_id` is set to null when the inviter's row is
/// deleted.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user" ADD COLUMN created_by_invite_id uuid NULL,
     ADD COLUMN invited_by_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL"#,
    r#"CREATE INDEX idx_user_invited_by_user_id ON "user" (invited_by_user_id)
     WHERE invited_by_user_id IS NOT NULL"#,
];

const DOWN_SQL: &[&str] = &[
    "DROP INDEX idx_user_invited_by_user_id",
    r#"ALTER TABLE "user" DROP COLUMN invited_by_user_id, DROP COLUMN created_by_invite_id"#,
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in UP_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
