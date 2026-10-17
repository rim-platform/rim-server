use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds a workspace's rules for direct messages to its members.
///
/// - `server.dm_outside_floor`: what the workspace allows its members at least for DMs from
///   outside it: `none` (no floor), `requests` or `closed`.
/// - `server.dm_member_default`: how a DM from a fellow member arrives unless the member chose:
///   `direct` or `requests`.
/// - `idx_relay_dm_request_pending`: the pending requests of a conversation, looked up for
///   every event of a DM that may be held.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE server ADD COLUMN dm_outside_floor text NOT NULL DEFAULT 'none', \
     ADD CONSTRAINT chk_server_dm_outside_floor \
     CHECK (dm_outside_floor IN ('none', 'requests', 'closed'))",
    "ALTER TABLE server ADD COLUMN dm_member_default text NOT NULL DEFAULT 'direct', \
     ADD CONSTRAINT chk_server_dm_member_default \
     CHECK (dm_member_default IN ('direct', 'requests'))",
    "CREATE INDEX idx_relay_dm_request_pending ON relay_dm_request (conversation_id) \
     WHERE status = 'pending'",
];

const DOWN_SQL: &[&str] = &[
    "DROP INDEX idx_relay_dm_request_pending",
    "ALTER TABLE server DROP CONSTRAINT chk_server_dm_member_default, \
     DROP COLUMN dm_member_default",
    "ALTER TABLE server DROP CONSTRAINT chk_server_dm_outside_floor, \
     DROP COLUMN dm_outside_floor",
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
