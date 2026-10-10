use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// Adds what a relay and a host need to carry direct messages between servers.
///
/// - `server.kind`: `workspace`, or the relay's one `direct` space holding the DMs and groups
///   its users start with anyone.
/// - `conversation.predecessor_*`: the conversation a successor replaces.
/// - `device_key.request_highest_seq`, `device_request_seen`: the `seq` window of device-signed
///   requests forwarded to this host.
/// - `host_event_stream`: the last `seq` of each event stream of a space.
/// - `s2s_outbox`, `s2s_peer_delivery`: signed host events waiting for each relay, and how far
///   each relay has acknowledged them.
/// - `s2s_peer_inbound`: how far this relay has received each host's outbox.
/// - `relay_envelope`, `relay_item`, `relay_device_cursor`: queued host events, one item per
///   recipient user, and how far each device has read.
/// - `relay_dm_request`, `relay_contact`, `relay_block`: the request inbox, contacts and blocks.
/// - `remote_identity_cache`: identities fetched from other relays.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE server ADD COLUMN kind text NOT NULL DEFAULT 'workspace', \
     ADD CONSTRAINT chk_server_kind CHECK (kind IN ('workspace', 'direct'))",
    "CREATE UNIQUE INDEX uq_server_direct ON server (kind) WHERE kind = 'direct'",
    "ALTER TABLE conversation ADD COLUMN predecessor_host text NULL, \
     ADD COLUMN predecessor_conversation_id uuid NULL",
    "ALTER TABLE device_key ADD COLUMN request_highest_seq bigint NULL",
    "CREATE TABLE device_request_seen (device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE, \
     seq bigint NOT NULL, expires_at timestamptz NOT NULL, PRIMARY KEY (device_key_id, seq))",
    "CREATE TABLE host_event_stream (workspace_id uuid NOT NULL REFERENCES server ON DELETE CASCADE, \
     stream text NOT NULL, seq bigint NOT NULL, PRIMARY KEY (workspace_id, stream))",
    "CREATE TABLE s2s_outbox (id bigserial PRIMARY KEY, peer_server_id text NOT NULL, item text NOT NULL, \
     size_bytes integer NOT NULL, created_at timestamptz NOT NULL DEFAULT now())",
    "CREATE INDEX idx_s2s_outbox_peer ON s2s_outbox (peer_server_id, id)",
    "CREATE TABLE s2s_peer_delivery (peer_server_id text PRIMARY KEY, delivered_through bigint NOT NULL DEFAULT 0, \
     failures integer NOT NULL DEFAULT 0, next_attempt_at timestamptz NOT NULL DEFAULT now(), \
     last_success_at timestamptz NULL)",
    "CREATE TABLE s2s_peer_inbound (peer_server_id text PRIMARY KEY, received_through bigint NOT NULL DEFAULT 0)",
    "CREATE TABLE relay_envelope (id bigserial PRIMARY KEY, host_server_id text NOT NULL, \
     workspace_id uuid NOT NULL, stream text NOT NULL, seq bigint NULL, payload text NOT NULL, \
     signature text NOT NULL, received_at timestamptz NOT NULL DEFAULT now())",
    "CREATE UNIQUE INDEX uq_relay_envelope_seq ON relay_envelope (host_server_id, workspace_id, stream, seq) \
     WHERE seq IS NOT NULL",
    r#"CREATE TABLE relay_dm_request (id uuid PRIMARY KEY,
     user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE, from_principal_id text NOT NULL,
     card_payload text NOT NULL, card_signature text NOT NULL, host_server_id text NOT NULL,
     conversation_id uuid NOT NULL,
     status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'ignored')),
     created_at timestamptz NOT NULL DEFAULT now(), decided_at timestamptz NULL,
     expires_at timestamptz NOT NULL, UNIQUE (user_id, host_server_id, conversation_id))"#,
    r#"CREATE TABLE relay_item (id bigserial PRIMARY KEY, user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     envelope_id bigint NOT NULL REFERENCES relay_envelope ON DELETE CASCADE,
     device_key_id uuid NULL REFERENCES device_key ON DELETE CASCADE,
     request_id uuid NULL REFERENCES relay_dm_request ON DELETE CASCADE,
     size_bytes integer NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
     expires_at timestamptz NOT NULL)"#,
    "CREATE INDEX idx_relay_item_user ON relay_item (user_id, id)",
    "CREATE INDEX idx_relay_item_expires ON relay_item (expires_at)",
    "CREATE TABLE relay_device_cursor (device_key_id uuid PRIMARY KEY REFERENCES device_key ON DELETE CASCADE, \
     acked_item_id bigint NOT NULL, updated_at timestamptz NOT NULL DEFAULT now())",
    r#"CREATE TABLE relay_contact (user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     principal_id text NOT NULL, relay_urls text[] NOT NULL,
     added_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (user_id, principal_id))"#,
    r#"CREATE TABLE relay_block (user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     principal_id text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
     PRIMARY KEY (user_id, principal_id))"#,
    "CREATE TABLE remote_identity_cache (principal_id text PRIMARY KEY, served_by text NOT NULL, \
     identity text NOT NULL, fetched_at timestamptz NOT NULL)",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE remote_identity_cache",
    "DROP TABLE relay_block",
    "DROP TABLE relay_contact",
    "DROP TABLE relay_device_cursor",
    "DROP TABLE relay_item",
    "DROP TABLE relay_dm_request",
    "DROP TABLE relay_envelope",
    "DROP TABLE s2s_peer_inbound",
    "DROP TABLE s2s_peer_delivery",
    "DROP TABLE s2s_outbox",
    "DROP TABLE host_event_stream",
    "DROP TABLE device_request_seen",
    "ALTER TABLE device_key DROP COLUMN request_highest_seq",
    "ALTER TABLE conversation DROP COLUMN predecessor_conversation_id, DROP COLUMN predecessor_host",
    "DELETE FROM server WHERE kind = 'direct'",
    "DROP INDEX uq_server_direct",
    "ALTER TABLE server DROP CONSTRAINT chk_server_kind, DROP COLUMN kind",
];

/// What must be gone before `down` may run: people from other servers, and queued items.
const DOWN_GUARD_SQL: &str = r#"SELECT (SELECT count(*) FROM "user" WHERE kind = 'remote')
    + (SELECT count(*) FROM relay_item) AS n"#;

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
                "remote users or queued relay items exist; remove them before reverting \
                 add_relay_delivery"
                    .to_string(),
            ));
        }
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
