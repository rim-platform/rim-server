use sea_orm_migration::prelude::*;

/// Key identity schema.
///
/// Additive only, so an operator can roll back by running `down`:
///
/// - `user.kind` (`local` | `remote`, default `local`), `user.principal_id`, `user.identity_key`
///   and `user.home_relay`. A person stays on their `user` row, so none of the tables that
///   reference `user.id` move when remote members get rows. `principal_id` is unique per server
///   and set together with `identity_key` when an account enrolls a key. `home_relay` and
///   `kind = 'remote'` are for remote members.
/// - `key_log_entry`: the principal's signed key log (inception, then rotations), one row per
///   `seq`. Payloads are stored exactly as signed, so any reader can verify them again.
/// - `device_key`: device keys certified by the identity key. A revoked row stays, so neither its
///   `device_id` nor its key can be enrolled again.
/// - `profile_card`: the latest signed card per user.
/// - `key_challenge`: single-use login challenges, short-lived.
/// - `session.device_key_id`: which device a key login session belongs to, so revoking the
///   device ends its sessions. Password sessions leave it null.
///
/// `password_hash` and `email` stay NOT NULL here: every enrolled account already has both.
/// `m20261009_105834_key_only_signup` makes them nullable.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user" ADD COLUMN kind text NOT NULL DEFAULT 'local'"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_kind CHECK (kind IN ('local', 'remote'))"#,
    r#"ALTER TABLE "user" ADD COLUMN principal_id text NULL"#,
    r#"ALTER TABLE "user" ADD COLUMN identity_key text NULL"#,
    r#"ALTER TABLE "user" ADD COLUMN home_relay text NULL"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal_has_identity_key
        CHECK ((principal_id IS NULL) = (identity_key IS NULL))"#,
    r#"CREATE UNIQUE INDEX idx_user_principal_id ON "user" (principal_id)"#,
    r#"CREATE TABLE key_log_entry (
        user_id uuid NOT NULL,
        seq integer NOT NULL,
        entry_type text NOT NULL,
        payload text NOT NULL,
        signature text NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        PRIMARY KEY (user_id, seq),
        CONSTRAINT chk_key_log_entry_seq CHECK (seq >= 0),
        CONSTRAINT chk_key_log_entry_type CHECK (entry_type IN ('inception', 'rotate')),
        CONSTRAINT fk_key_log_entry_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    r#"CREATE TABLE device_key (
        id uuid PRIMARY KEY,
        user_id uuid NOT NULL,
        device_id uuid NOT NULL,
        public_key text NOT NULL,
        name text NOT NULL,
        cert_payload text NOT NULL,
        cert_signature text NOT NULL,
        key_seq integer NOT NULL,
        added_at timestamptz NOT NULL DEFAULT now(),
        last_used_at timestamptz NULL,
        revoked_at timestamptz NULL,
        revocation_payload text NULL,
        revocation_signature text NULL,
        CONSTRAINT uq_device_key_user_device UNIQUE (user_id, device_id),
        CONSTRAINT uq_device_key_user_public_key UNIQUE (user_id, public_key),
        CONSTRAINT fk_device_key_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    r#"CREATE TABLE profile_card (
        user_id uuid PRIMARY KEY,
        version bigint NOT NULL,
        payload text NOT NULL,
        signature text NOT NULL,
        updated_at timestamptz NOT NULL DEFAULT now(),
        CONSTRAINT fk_profile_card_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    r#"CREATE TABLE key_challenge (
        id uuid PRIMARY KEY,
        nonce text NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        expires_at timestamptz NOT NULL,
        used_at timestamptz NULL
    )"#,
    "CREATE INDEX idx_key_challenge_expires_at ON key_challenge (expires_at)",
    r#"ALTER TABLE session ADD COLUMN device_key_id uuid NULL
        CONSTRAINT fk_session_device_key REFERENCES device_key (id)
        ON DELETE SET NULL ON UPDATE CASCADE"#,
    "CREATE INDEX idx_session_device_key_id ON session (device_key_id) \
     WHERE device_key_id IS NOT NULL",
];

/// Drops everything `up` added. Enrolled keys, device lists and cards are lost; accounts keep
/// their passwords, so everyone can still sign in with a password afterwards.
const DOWN_SQL: &[&str] = &[
    "DROP INDEX idx_session_device_key_id",
    "ALTER TABLE session DROP COLUMN device_key_id",
    "DROP TABLE key_challenge",
    "DROP TABLE profile_card",
    "DROP TABLE device_key",
    "DROP TABLE key_log_entry",
    "DROP INDEX idx_user_principal_id",
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal_has_identity_key"#,
    r#"ALTER TABLE "user" DROP COLUMN home_relay"#,
    r#"ALTER TABLE "user" DROP COLUMN identity_key"#,
    r#"ALTER TABLE "user" DROP COLUMN principal_id"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_kind"#,
    r#"ALTER TABLE "user" DROP COLUMN kind"#,
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
