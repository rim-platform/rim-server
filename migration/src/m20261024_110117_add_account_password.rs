use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds the account password, checked with OPAQUE, and the session columns that go with it.
///
/// - `account_password`: one row per local account with a password. `registration` is the
///   OPAQUE registration record; `share_sealed` the account's server share, sealed under the
///   server's password key. `failed_attempts`, `next_attempt_at` and `locked_at` count the tries
///   since the last success, when the next one is allowed, and when the tenth locked the
///   password until a reset with the recovery key.
/// - `password_login`: the server's OPAQUE login state between the start and the finish of a
///   sign-in (`login`) or a password change (`change`), for 120 seconds; the finish deletes it.
/// - `password_ticket`: proof that a password login finished, which the key sign-in that
///   follows consumes: the SHA-256 hex of the ticket, the account, and an expiry 120 seconds out.
/// - `password_rewrap`: the previous unlock key sealed under the new one, written by each
///   password change; the newest three per account are kept.
/// - `password_reset`: a reset proved with the recovery key, between its start and its finish.
/// - `session`: `kept`, `secret_hash`, `keep_key_sealed`, `reload_key_sealed` and
///   `ws_closed_at` for "Keep me signed in" and the reload window, and `must_set_password`,
///   which limits a session of an account without a password to setting one.
/// - `key_challenge.purpose` gains `password_reset`.
///
/// New tables, and columns that are nullable or have a default: the previous release's server
/// neither writes nor reads them.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE account_password (
    user_id uuid PRIMARY KEY REFERENCES "user" (id) ON DELETE CASCADE,
    registration bytea NOT NULL,
    share_sealed bytea NOT NULL,
    failed_attempts int NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NULL,
    locked_at timestamptz NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
)"#,
    r#"CREATE TABLE password_login (
    id uuid PRIMARY KEY,
    user_id uuid NULL REFERENCES "user" (id) ON DELETE CASCADE,
    purpose text NOT NULL CHECK (purpose IN ('login', 'change')),
    state bytea NOT NULL,
    expires_at timestamptz NOT NULL
)"#,
    "CREATE INDEX idx_password_login_expires ON password_login (expires_at)",
    r#"CREATE TABLE password_ticket (
    token_hash text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL
)"#,
    "CREATE INDEX idx_password_ticket_expires ON password_ticket (expires_at)",
    r#"CREATE TABLE password_rewrap (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    rewrap bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
)"#,
    "CREATE INDEX idx_password_rewrap_user ON password_rewrap (user_id, created_at)",
    r#"CREATE TABLE password_reset (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
    state bytea NULL,
    expires_at timestamptz NOT NULL
)"#,
    "CREATE INDEX idx_password_reset_expires ON password_reset (expires_at)",
    "ALTER TABLE session ADD COLUMN kept boolean NOT NULL DEFAULT true",
    "ALTER TABLE session ADD COLUMN secret_hash text NULL",
    "ALTER TABLE session ADD COLUMN keep_key_sealed bytea NULL",
    "ALTER TABLE session ADD COLUMN reload_key_sealed bytea NULL",
    "ALTER TABLE session ADD COLUMN ws_closed_at timestamptz NULL",
    "ALTER TABLE session ADD COLUMN must_set_password boolean NOT NULL DEFAULT false",
    "ALTER TABLE key_challenge DROP CONSTRAINT chk_key_challenge_purpose",
    "ALTER TABLE key_challenge ADD CONSTRAINT chk_key_challenge_purpose \
     CHECK ((purpose IN ('login', 'password_reset') AND relay_server_id IS NULL) \
     OR (purpose = 'join' AND relay_server_id IS NOT NULL))",
];

const DOWN_SQL: &[&str] = &[
    "DELETE FROM key_challenge WHERE purpose = 'password_reset'",
    "ALTER TABLE key_challenge DROP CONSTRAINT chk_key_challenge_purpose",
    "ALTER TABLE key_challenge ADD CONSTRAINT chk_key_challenge_purpose \
     CHECK ((purpose = 'login' AND relay_server_id IS NULL) \
     OR (purpose = 'join' AND relay_server_id IS NOT NULL))",
    "ALTER TABLE session DROP COLUMN must_set_password",
    "ALTER TABLE session DROP COLUMN ws_closed_at",
    "ALTER TABLE session DROP COLUMN reload_key_sealed",
    "ALTER TABLE session DROP COLUMN keep_key_sealed",
    "ALTER TABLE session DROP COLUMN secret_hash",
    "ALTER TABLE session DROP COLUMN kept",
    "DROP TABLE password_reset",
    "DROP TABLE password_rewrap",
    "DROP TABLE password_ticket",
    "DROP TABLE password_login",
    "DROP TABLE account_password",
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
