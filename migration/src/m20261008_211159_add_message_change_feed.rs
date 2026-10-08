use sea_orm_migration::prelude::*;

/// Message change feed (master plan PR 8, `docs/message-change-feed.md`).
///
/// Additive only, so an operator can roll back by running `down`:
///
/// - `message.version bigint`: the message's position in its conversation's change sequence.
///   Indexed on `(conversation_id, version)` for the feed query.
/// - `message.edited_at timestamptz NULL`: when the text last changed.
/// - `message_change_seq`: one counter row per conversation (`conversation_id`, `last_version`).
/// - Trigger `message_change_feed` (BEFORE INSERT, and BEFORE UPDATE when `text`, `is_deleted`
///   or `has_attachments` changes) takes the next value from the conversation's counter and
///   writes it to `version`, and sets `edited_at` when the text changed. Doing this in the
///   database covers every write path (send, reply, forward, edit, delete, attachments, and any
///   future one) with no service code to forget.
///
/// **Why a per-conversation counter row, not a global sequence:** the upsert locks the counter
/// row until the writing transaction commits, so a second writer in the same conversation waits
/// and gets the next number only after the first commits (or reuses it after a rollback). A
/// reader therefore never sees version N+1 while N is still in flight, and a client that stores
/// "last version seen" never skips a change. A global sequence hands out numbers at insert time,
/// not commit time, so a later number can become visible first. Sends then also lock the
/// conversation row (`last_activity_at`) until commit, so the extra lock costs little.
///
/// **Data step:** existing messages get versions `1..n` per conversation in `(sent_at, id)`
/// order, and each conversation's counter starts at its `n`. A client syncing from the
/// beginning therefore receives the full history. See [`BACKFILL_VERSIONS_SQL`]. The back-fill
/// rewrites every message row (and its `search_tsv`) while the `ALTER TABLE` lock blocks reads
/// and writes of `message`, so on a large table run it in a maintenance window.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The data step: number existing messages per conversation. Covered by the `rim_infra_db`
/// db-test `message_change_feed::migration_numbers_existing_messages_per_conversation_by_sent_at`.
const BACKFILL_VERSIONS_SQL: &str = r#"UPDATE message m
    SET version = numbered.rn
    FROM (
        SELECT id,
               row_number() OVER (PARTITION BY conversation_id ORDER BY sent_at, id) AS rn
        FROM message
    ) AS numbered
    WHERE m.id = numbered.id"#;

const UP_SQL: &[&str] = &[
    "ALTER TABLE message ADD COLUMN version bigint NOT NULL DEFAULT 0",
    "ALTER TABLE message ADD COLUMN edited_at timestamptz NULL",
    r#"CREATE TABLE message_change_seq (
        conversation_id uuid PRIMARY KEY,
        last_version bigint NOT NULL,
        CONSTRAINT fk_message_change_seq_conversation FOREIGN KEY (conversation_id)
            REFERENCES conversation (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    BACKFILL_VERSIONS_SQL,
    "INSERT INTO message_change_seq (conversation_id, last_version) \
     SELECT conversation_id, max(version) FROM message GROUP BY conversation_id",
    // Every row has a real version now; without a default, an insert that bypasses the trigger
    // (`session_replication_role = replica`, a manual repair) fails instead of landing at
    // version 0, where the feed would never return it.
    "ALTER TABLE message ALTER COLUMN version DROP DEFAULT",
    "CREATE INDEX idx_message_conversation_version ON message (conversation_id, version)",
    r#"CREATE FUNCTION message_change_feed() RETURNS trigger
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
    $$"#,
    "CREATE TRIGGER message_change_feed_insert BEFORE INSERT ON message \
     FOR EACH ROW EXECUTE FUNCTION message_change_feed()",
    "CREATE TRIGGER message_change_feed_update BEFORE UPDATE ON message \
     FOR EACH ROW WHEN ( \
         NEW.text IS DISTINCT FROM OLD.text \
         OR NEW.is_deleted IS DISTINCT FROM OLD.is_deleted \
         OR NEW.has_attachments IS DISTINCT FROM OLD.has_attachments \
     ) EXECUTE FUNCTION message_change_feed()",
];

/// Drops the triggers, the function, the counter table and the two columns. Versions and edit
/// times are lost; clients re-sync from the beginning after the migration runs again.
const DOWN_SQL: &[&str] = &[
    "DROP TRIGGER message_change_feed_update ON message",
    "DROP TRIGGER message_change_feed_insert ON message",
    "DROP FUNCTION message_change_feed()",
    "DROP INDEX idx_message_conversation_version",
    "DROP TABLE message_change_seq",
    "ALTER TABLE message DROP COLUMN edited_at",
    "ALTER TABLE message DROP COLUMN version",
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
