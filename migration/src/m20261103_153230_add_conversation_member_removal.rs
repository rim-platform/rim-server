use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds `conversation_member_removal`: one row each time an Owner or Moderator removed someone
/// else from a channel, with who removed them and when, for the channel's timeline. New table:
/// the previous release's server never reads it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE conversation_member_removal (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    removed_by uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    removed_at timestamptz NOT NULL DEFAULT now()
)"#,
    "CREATE INDEX idx_conversation_member_removal_conversation \
     ON conversation_member_removal (conversation_id, removed_at DESC, id DESC)",
];

const DOWN_SQL: &[&str] = &["DROP TABLE conversation_member_removal"];

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
