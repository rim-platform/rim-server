use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// End-to-end encrypted DMs and groups with MLS.
///
/// - `conversation.encrypted_at`: set once a conversation is encrypted, never cleared.
/// - `device_key.last_key_package_upload_at`: a device's MLS activity signal. Key package rows
///   are deleted when they expire, so the time of the last upload is kept on the device.
/// - `franking_key`: the server's HMAC keys for franking receipts, created on first use.
/// - `mls_key_package`: one-time and last-resort key packages per device, claimed at most once
///   (one-time) through `FOR UPDATE SKIP LOCKED`.
/// - `mls_group`, `mls_group_device`: one group per conversation and incarnation, the epoch the
///   next Commit must be created in, and the devices that are its leaves.
/// - `mls_commit`: one Commit per `(conversation, incarnation, epoch)`.
/// - `mls_welcome`, `mls_welcome_recipient`: Welcomes until each added device acknowledges.
/// - `mls_readd_request`: signed requests of devices that lost their group state.
/// - `message`: the MLS envelope of encrypted rows, whose `text` is always null.
/// - `message_attachment.blob_sha256`, `message_report.franked_content` and
///   `franking_verified_at`.
///
/// The change feed's function also stamps `edited_at` when an envelope is replaced, and its
/// update trigger fires on that change.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The change feed function with envelope edits. Clearing the envelope on delete is not an edit.
const FEED_FUNCTION_SQL: &str = r#"CREATE OR REPLACE FUNCTION message_change_feed() RETURNS trigger
    LANGUAGE plpgsql AS $$
    BEGIN
        INSERT INTO message_change_seq AS s (conversation_id, last_version)
        VALUES (NEW.conversation_id, 1)
        ON CONFLICT (conversation_id)
            DO UPDATE SET last_version = s.last_version + 1
        RETURNING s.last_version INTO NEW.version;
        IF TG_OP = 'UPDATE' AND (
            NEW.text IS DISTINCT FROM OLD.text
            OR (NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext
                AND NEW.mls_ciphertext IS NOT NULL)
        ) THEN
            NEW.edited_at := now();
        END IF;
        RETURN NEW;
    END
    $$"#;

/// The change feed function as it was before this migration.
const PREVIOUS_FEED_FUNCTION_SQL: &str = r#"CREATE OR REPLACE FUNCTION message_change_feed() RETURNS trigger
    LANGUAGE plpgsql AS $$
    BEGIN
        INSERT INTO message_change_seq AS s (conversation_id, last_version)
        VALUES (NEW.conversation_id, 1)
        ON CONFLICT (conversation_id)
            DO UPDATE SET last_version = s.last_version + 1
        RETURNING s.last_version INTO NEW.version;
        IF TG_OP = 'UPDATE' AND NEW.text IS DISTINCT FROM OLD.text THEN
            NEW.edited_at := now();
        END IF;
        RETURN NEW;
    END
    $$"#;

const UP_SQL: &[&str] = &[
    "ALTER TABLE conversation ADD COLUMN encrypted_at timestamptz NULL",
    "ALTER TABLE device_key ADD COLUMN last_key_package_upload_at timestamptz NULL",
    r#"CREATE TABLE franking_key (
        id integer PRIMARY KEY,
        secret bytea NOT NULL CHECK (octet_length(secret) = 32),
        created_at timestamptz NOT NULL DEFAULT now()
    )"#,
    r#"CREATE TABLE mls_key_package (
        id uuid PRIMARY KEY,
        user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
        device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
        cipher_suite integer NOT NULL,
        key_package_ref bytea NOT NULL,
        key_package bytea NOT NULL,
        last_resort boolean NOT NULL DEFAULT false,
        not_after timestamptz NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        claimed_at timestamptz NULL,
        claimed_by_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
        claimed_for_conversation_id uuid NULL REFERENCES conversation (id) ON DELETE SET NULL,
        claimed_for_incarnation integer NULL,
        claim_count integer NOT NULL DEFAULT 0,
        CONSTRAINT uq_mls_key_package_ref UNIQUE (key_package_ref)
    )"#,
    "CREATE INDEX idx_mls_key_package_available \
     ON mls_key_package (device_key_id, cipher_suite, created_at) \
     WHERE claimed_at IS NULL AND NOT last_resort",
    "CREATE UNIQUE INDEX uq_mls_key_package_last_resort \
     ON mls_key_package (device_key_id, cipher_suite) WHERE last_resort",
    "CREATE INDEX idx_mls_key_package_not_after ON mls_key_package (not_after)",
    r#"CREATE TABLE mls_group (
        conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
        incarnation integer NOT NULL CHECK (incarnation >= 1),
        group_id bytea NOT NULL,
        cipher_suite integer NOT NULL,
        epoch bigint NOT NULL,
        created_by_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        retired_at timestamptz NULL,
        PRIMARY KEY (conversation_id, incarnation),
        CONSTRAINT uq_mls_group_group_id UNIQUE (group_id)
    )"#,
    "CREATE UNIQUE INDEX uq_mls_group_current ON mls_group (conversation_id) \
     WHERE retired_at IS NULL",
    r#"CREATE TABLE mls_group_device (
        conversation_id uuid NOT NULL,
        incarnation integer NOT NULL,
        device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
        user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
        added_epoch bigint NOT NULL,
        PRIMARY KEY (conversation_id, incarnation, device_key_id),
        FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE
    )"#,
    r#"CREATE TABLE mls_commit (
        id uuid PRIMARY KEY,
        conversation_id uuid NOT NULL,
        incarnation integer NOT NULL,
        epoch bigint NOT NULL,
        sender_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL,
        sender_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
        message bytea NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE,
        CONSTRAINT uq_mls_commit_epoch UNIQUE (conversation_id, incarnation, epoch)
    )"#,
    r#"CREATE TABLE mls_welcome (
        id uuid PRIMARY KEY,
        commit_id uuid NOT NULL REFERENCES mls_commit (id) ON DELETE CASCADE,
        conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
        incarnation integer NOT NULL,
        epoch bigint NOT NULL,
        message bytea NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        expires_at timestamptz NOT NULL
    )"#,
    r#"CREATE TABLE mls_welcome_recipient (
        welcome_id uuid NOT NULL REFERENCES mls_welcome (id) ON DELETE CASCADE,
        device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
        user_id uuid NOT NULL REFERENCES "user" (id) ON DELETE CASCADE,
        acked_at timestamptz NULL,
        PRIMARY KEY (welcome_id, device_key_id)
    )"#,
    "CREATE INDEX idx_mls_welcome_recipient_device ON mls_welcome_recipient (device_key_id) \
     WHERE acked_at IS NULL",
    r#"CREATE TABLE mls_readd_request (
        id uuid PRIMARY KEY,
        conversation_id uuid NOT NULL,
        incarnation integer NOT NULL,
        device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
        payload text NOT NULL,
        signature text NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        fulfilled_epoch bigint NULL,
        FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE
    )"#,
    r#"ALTER TABLE message
        ADD COLUMN encrypted boolean NOT NULL DEFAULT false,
        ADD COLUMN mls_ciphertext bytea NULL,
        ADD COLUMN mls_incarnation integer NULL,
        ADD COLUMN mls_epoch bigint NULL,
        ADD COLUMN sender_device_key_id uuid NULL REFERENCES device_key (id) ON DELETE SET NULL,
        ADD COLUMN franking_commitment bytea NULL,
        ADD COLUMN franking_receipt bytea NULL,
        ADD COLUMN franking_key_id integer NULL REFERENCES franking_key (id),
        ADD CONSTRAINT chk_message_encrypted_text CHECK (NOT encrypted OR text IS NULL),
        ADD CONSTRAINT chk_message_encrypted_envelope
            CHECK (encrypted = (mls_epoch IS NOT NULL AND mls_incarnation IS NOT NULL))"#,
    "ALTER TABLE message_attachment ADD COLUMN blob_sha256 bytea NULL",
    "ALTER TABLE message_report \
     ADD COLUMN franked_content bytea NULL, \
     ADD COLUMN franking_verified_at timestamptz NULL",
    FEED_FUNCTION_SQL,
    "DROP TRIGGER message_change_feed_update ON message",
    "CREATE TRIGGER message_change_feed_update BEFORE UPDATE ON message \
     FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
         OR NEW.mls_ciphertext IS DISTINCT FROM OLD.mls_ciphertext \
     ) EXECUTE FUNCTION message_change_feed()",
];

/// Restores the previous change feed function and trigger, then drops everything `up` added.
/// Group state, key packages and verified report contents are lost.
const DOWN_SQL: &[&str] = &[
    "DROP TRIGGER message_change_feed_update ON message",
    "CREATE TRIGGER message_change_feed_update BEFORE UPDATE ON message \
     FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
     ) EXECUTE FUNCTION message_change_feed()",
    PREVIOUS_FEED_FUNCTION_SQL,
    "ALTER TABLE message_report DROP COLUMN franking_verified_at, DROP COLUMN franked_content",
    "ALTER TABLE message_attachment DROP COLUMN blob_sha256",
    r#"ALTER TABLE message
        DROP CONSTRAINT chk_message_encrypted_envelope,
        DROP CONSTRAINT chk_message_encrypted_text,
        DROP COLUMN franking_key_id,
        DROP COLUMN franking_receipt,
        DROP COLUMN franking_commitment,
        DROP COLUMN sender_device_key_id,
        DROP COLUMN mls_epoch,
        DROP COLUMN mls_incarnation,
        DROP COLUMN mls_ciphertext,
        DROP COLUMN encrypted"#,
    "DROP TABLE mls_readd_request",
    "DROP TABLE mls_welcome_recipient",
    "DROP TABLE mls_welcome",
    "DROP TABLE mls_commit",
    "DROP TABLE mls_group_device",
    "DROP TABLE mls_group",
    "DROP TABLE mls_key_package",
    "DROP TABLE franking_key",
    "ALTER TABLE device_key DROP COLUMN last_key_package_upload_at",
    "ALTER TABLE conversation DROP COLUMN encrypted_at",
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

    /// Refused while an encrypted message exists: rolling back would leave rows with neither
    /// text nor ciphertext.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let encrypted = conn
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT EXISTS (SELECT 1 FROM message WHERE encrypted) AS present",
            ))
            .await?
            .map(|row| row.try_get::<bool>("", "present"))
            .transpose()?
            .unwrap_or(false);
        if encrypted {
            return Err(DbErr::Migration(
                "encrypted messages exist; delete them before rolling back add_e2ee_mls"
                    .to_string(),
            ));
        }
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
