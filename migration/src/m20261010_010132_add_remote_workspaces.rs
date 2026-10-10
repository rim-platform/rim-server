use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// Adds what a host and a relay need for workspace members whose relay is another server.
///
/// - `host_delegation`: delegations a member's devices signed for a relay, one row per
///   workspace, device and relay; the relay receives the workspace's events for the member
///   while one is active.
/// - `relay_subscription`: this relay's subscriptions to workspaces on other hosts, per user.
/// - `relay_revocation_fanout`: a revoked device's revocation sent to each host holding its
///   person, with the host's acknowledgement.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE host_delegation (id uuid PRIMARY KEY,
     workspace_id uuid NOT NULL REFERENCES server ON DELETE CASCADE,
     user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE,
     relay_server_id text NOT NULL, payload text NOT NULL, signature text NOT NULL,
     accepted_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL,
     ended_at timestamptz NULL, UNIQUE (workspace_id, device_key_id, relay_server_id))"#,
    "CREATE INDEX idx_host_delegation_active ON host_delegation (workspace_id, relay_server_id, user_id) \
     WHERE ended_at IS NULL",
    r#"CREATE TABLE relay_subscription (id uuid PRIMARY KEY,
     user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     host_server_id text NOT NULL, workspace_id uuid NOT NULL,
     status text NOT NULL CHECK (status IN ('active', 'lapsed', 'ended')), expires_at timestamptz NULL,
     updated_at timestamptz NOT NULL DEFAULT now(), UNIQUE (user_id, host_server_id, workspace_id))"#,
    "CREATE TABLE relay_revocation_fanout (device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE, \
     host_server_id text NOT NULL, status text NOT NULL CHECK (status IN ('pending', 'acked', 'gave_up')), \
     attempts integer NOT NULL DEFAULT 0, next_attempt_at timestamptz NOT NULL, \
     ack_payload text NULL, ack_signature text NULL, PRIMARY KEY (device_key_id, host_server_id))",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE relay_revocation_fanout",
    "DROP TABLE relay_subscription",
    "DROP TABLE host_delegation",
];

/// What must be gone before `down` may run: delegations, subscriptions and revocations still
/// being sent.
const DOWN_GUARD_SQL: &str = "SELECT (SELECT count(*) FROM host_delegation) \
    + (SELECT count(*) FROM relay_subscription) \
    + (SELECT count(*) FROM relay_revocation_fanout) AS n";

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
        let row = conn
            .query_one_raw(Statement::from_string(DbBackend::Postgres, DOWN_GUARD_SQL))
            .await?
            .ok_or_else(|| DbErr::Custom("the down guard returned no row".to_string()))?;
        let remaining: i64 = row.try_get("", "n")?;
        if remaining > 0 {
            return Err(DbErr::Migration(
                "delegations, relay subscriptions or revocation fan-outs exist; remove them \
                 before reverting add_remote_workspaces"
                    .to_string(),
            ));
        }
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
