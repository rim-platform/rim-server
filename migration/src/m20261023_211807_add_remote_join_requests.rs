use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds what join requests from people of other servers need, on both sides.
///
/// - `workspace_join_request.relay_server_id`: on a host, the server id of the relay that filed
///   a request for a person of another server; null for this server's accounts. The decision
///   is delivered to that relay.
/// - `relay_join_request`: on a relay, a request a local user filed at a workspace on another
///   host, until it expires (`expires_at`, the host's) or its decision is delivered. The relay
///   queues the decision for the user only while the row exists.
///
/// A nullable column and a new table: the previous release's server neither writes nor reads
/// them.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE workspace_join_request ADD COLUMN relay_server_id text NULL",
    r#"CREATE TABLE relay_join_request (
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    host_server_id text NOT NULL,
    workspace_id uuid NOT NULL,
    request_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (user_id, host_server_id, workspace_id)
)"#,
    "CREATE INDEX idx_relay_join_request_expires ON relay_join_request (expires_at)",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE relay_join_request",
    "ALTER TABLE workspace_join_request DROP COLUMN relay_server_id",
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
