use sea_orm_migration::prelude::*;

/// Adds the server's identity and what it knows of its peers.
///
/// - `server_key_log_entry`: this server's key log, inception at seq 0. Private keys are never
///   stored; the server key's seed lives in the key file.
/// - `server_descriptor`: the one signed descriptor this server publishes (`id = 1`).
/// - `peer_server`, `peer_key_log_entry`: each pinned peer, its verified key log, head and
///   previous key, descriptor and status (`active`, `forked`, `blocked`).
/// - `s2s_nonce`: request nonces seen per peer until `expires_at`; the primary key turns a
///   replay into a conflict.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE server_key_log_entry (
        seq integer PRIMARY KEY CHECK (seq >= 0),
        entry_type text NOT NULL CHECK (entry_type IN ('inception', 'rotate')),
        payload text NOT NULL,
        signature text NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now()
    )"#,
    r#"CREATE TABLE server_descriptor (
        id smallint PRIMARY KEY CHECK (id = 1),
        version integer NOT NULL,
        payload text NOT NULL,
        signature text NOT NULL,
        updated_at timestamptz NOT NULL DEFAULT now()
    )"#,
    r#"CREATE TABLE peer_server (
        server_id text PRIMARY KEY,
        origin text NOT NULL,
        head_seq integer NOT NULL,
        head_key text NOT NULL,
        previous_key text NULL,
        rotated_at timestamptz NULL,
        rotation_signer text NULL CHECK (rotation_signer IN ('server', 'recovery')),
        descriptor_version integer NOT NULL,
        descriptor_payload text NOT NULL,
        descriptor_signature text NOT NULL,
        status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'forked', 'blocked')),
        first_seen_at timestamptz NOT NULL DEFAULT now(),
        fetched_at timestamptz NOT NULL,
        last_request_at timestamptz NULL
    )"#,
    "CREATE INDEX idx_peer_server_origin ON peer_server (origin)",
    r#"CREATE TABLE peer_key_log_entry (
        server_id text NOT NULL REFERENCES peer_server ON DELETE CASCADE,
        seq integer NOT NULL,
        payload text NOT NULL,
        signature text NOT NULL,
        PRIMARY KEY (server_id, seq)
    )"#,
    r#"CREATE TABLE s2s_nonce (
        peer_server_id text NOT NULL,
        nonce text NOT NULL,
        expires_at timestamptz NOT NULL,
        PRIMARY KEY (peer_server_id, nonce)
    )"#,
    "CREATE INDEX idx_s2s_nonce_expires ON s2s_nonce (expires_at)",
];

/// Drops every table. The key file stays on disk; a server brought back up refuses to start
/// until its key log is restored or the file is moved away (a new identity).
const DOWN_SQL: &[&str] = &[
    "DROP TABLE s2s_nonce",
    "DROP TABLE peer_key_log_entry",
    "DROP TABLE peer_server",
    "DROP TABLE server_descriptor",
    "DROP TABLE server_key_log_entry",
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
