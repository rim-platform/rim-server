use sea_orm_migration::prelude::*;

/// Key identity algorithm agility: stored keys carry their algorithm.
///
/// - `device_key.public_key` and `user.identity_key` move to the tagged text form
///   (`ed25519:<base64url>`), so a column that will hold keys of several algorithms says which
///   one each key is. Every key stored so far is a bare Ed25519 key; base64url has no `:`, so a
///   value without one is bare.
/// - `device_key.key_hash` (SHA-256 of the tagged text, 32 bytes) replaces the unique index on
///   the raw public key: a hybrid Ed25519 + ML-DSA key is about 1.8 KB of text, close to the
///   btree row limit, and the index only needs equality. Existing rows get their hash here,
///   computed the same way as `rim_identity::PublicKey::key_hash`.
///
/// Signed payloads are not touched: statements stay exactly as signed.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"UPDATE device_key SET public_key = 'ed25519:' || public_key
        WHERE strpos(public_key, ':') = 0"#,
    r#"UPDATE "user" SET identity_key = 'ed25519:' || identity_key
        WHERE identity_key IS NOT NULL AND strpos(identity_key, ':') = 0"#,
    "ALTER TABLE device_key ADD COLUMN key_hash bytea NULL",
    "UPDATE device_key SET key_hash = sha256(convert_to(public_key, 'UTF8'))",
    "ALTER TABLE device_key ALTER COLUMN key_hash SET NOT NULL",
    "ALTER TABLE device_key ADD CONSTRAINT chk_device_key_key_hash_len \
     CHECK (octet_length(key_hash) = 32)",
    "ALTER TABLE device_key DROP CONSTRAINT uq_device_key_user_public_key",
    "ALTER TABLE device_key ADD CONSTRAINT uq_device_key_user_key_hash UNIQUE (user_id, key_hash)",
];

/// Back to bare Ed25519 keys and the raw-key index. Refuses to run once a key of another
/// algorithm is stored, because the previous schema and server cannot read it.
const DOWN_SQL: &[&str] = &[
    r#"DO $$
    BEGIN
        IF EXISTS (SELECT 1 FROM device_key WHERE public_key NOT LIKE 'ed25519:%')
            OR EXISTS (SELECT 1 FROM "user"
                       WHERE identity_key IS NOT NULL AND identity_key NOT LIKE 'ed25519:%')
        THEN
            RAISE EXCEPTION 'non-Ed25519 keys are stored; the previous schema cannot hold them';
        END IF;
    END
    $$"#,
    "ALTER TABLE device_key DROP CONSTRAINT uq_device_key_user_key_hash",
    "ALTER TABLE device_key DROP CONSTRAINT chk_device_key_key_hash_len",
    "ALTER TABLE device_key DROP COLUMN key_hash",
    "UPDATE device_key SET public_key = substr(public_key, 9)",
    r#"UPDATE "user" SET identity_key = substr(identity_key, 9) WHERE identity_key IS NOT NULL"#,
    "ALTER TABLE device_key ADD CONSTRAINT uq_device_key_user_public_key \
     UNIQUE (user_id, public_key)",
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
