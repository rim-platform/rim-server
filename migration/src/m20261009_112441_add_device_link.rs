use sea_orm_migration::prelude::*;

/// Adds `device_link`: the links a signed-in device starts so a new device can get its own
/// certified device key, with the server relaying the sealed offer and answer.
///
/// - `link_hash` is the SHA-256 of the 16-byte link id. The id itself is a bearer capability
///   (post an offer, read the answer, cancel), so a database read must not hand it out.
/// - `state` moves `open` → `offered` → `answered` → `consumed`; `cancelled` ends any live
///   state. Expiry is `expires_at`, never a state.
/// - The sealed columns hold the bytes as received; `consumed` and `cancelled` clear them.
/// - `uq_device_link_live_user` allows one live-state link per user; starting a new one
///   cancels the previous in the same transaction.
///
/// Rows past their expiry by more than an hour are deleted when links are started.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE device_link (
        id uuid PRIMARY KEY,
        link_hash bytea NOT NULL,
        user_id uuid NOT NULL,
        state text NOT NULL DEFAULT 'open',
        offer_nonce bytea NULL,
        offer_ciphertext bytea NULL,
        answer_nonce bytea NULL,
        answer_ciphertext bytea NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        expires_at timestamptz NOT NULL,
        offered_at timestamptz NULL,
        answered_at timestamptz NULL,
        ended_at timestamptz NULL,
        CONSTRAINT uq_device_link_link_hash UNIQUE (link_hash),
        CONSTRAINT chk_device_link_link_hash CHECK (octet_length(link_hash) = 32),
        CONSTRAINT chk_device_link_state
            CHECK (state IN ('open', 'offered', 'answered', 'consumed', 'cancelled')),
        CONSTRAINT chk_device_link_offer CHECK ((offer_nonce IS NULL) = (offer_ciphertext IS NULL)),
        CONSTRAINT chk_device_link_answer
            CHECK ((answer_nonce IS NULL) = (answer_ciphertext IS NULL)),
        CONSTRAINT fk_device_link_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    "CREATE INDEX idx_device_link_expires_at ON device_link (expires_at)",
    "CREATE UNIQUE INDEX uq_device_link_live_user ON device_link (user_id) \
     WHERE state IN ('open', 'offered', 'answered')",
];

/// Nothing references the table. What it loses is links at most 20 minutes old: a person
/// mid-link starts again. Devices already added stay in `device_key`.
const DOWN_SQL: &[&str] = &[
    "DROP INDEX uq_device_link_live_user",
    "DROP INDEX idx_device_link_expires_at",
    "DROP TABLE device_link",
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
