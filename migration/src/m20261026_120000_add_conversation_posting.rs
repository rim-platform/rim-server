use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Per-channel posting permissions.
///
/// - `conversation.posting_policy` — who may post in the channel: `everyone` (default) or
///   `moderators` (the workspace's Owner and Moderators). Text with a CHECK
///   (`conversation_posting_policy_check`) so further policies only widen the constraint.
/// - `conversation.posting_allows_threads` — under `moderators`, whether everyone else may still
///   react and reply in threads (default `true`).
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE conversation ADD COLUMN posting_policy text NOT NULL DEFAULT 'everyone', \
     ADD CONSTRAINT conversation_posting_policy_check \
     CHECK (posting_policy IN ('everyone', 'moderators'))",
    "ALTER TABLE conversation ADD COLUMN posting_allows_threads boolean NOT NULL DEFAULT true",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE conversation DROP COLUMN posting_allows_threads",
    "ALTER TABLE conversation DROP CONSTRAINT conversation_posting_policy_check, \
     DROP COLUMN posting_policy",
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
