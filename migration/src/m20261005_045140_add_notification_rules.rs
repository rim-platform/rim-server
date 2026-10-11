use sea_orm_migration::prelude::*;

/// Server-side notification rules.
///
/// - Enums `notification_level` (`all`, `mentions`, `none`), `digest_frequency` (`off`,
///   `hourly`, `daily`) and `notification_reason` (`direct`, `mention`, `keyword`, `all`,
///   `thread_reply`).
/// - `notification_preference.level` (nullable `notification_level`): the per-conversation
///   override; null inherits the workspace default. `muted_until` stays (a timed `none`).
/// - `user_notification_settings`, primary key `(user_id, server_id)`: the workspace default
///   level, keyword alerts, the do-not-disturb window (`dnd_start`/`dnd_end`/`dnd_timezone`, all
///   set or all null), a manual do-not-disturb pause (`dnd_paused`, optionally ending at
///   `dnd_paused_until`, used by the `notifications.dnd` command) and the email digest frequency.
/// - `user_notification`: one row per `(user_id, message_id)` (unique), indexed on
///   `(user_id, read_at, created_at DESC)` for the inbox and on `message_id` for the
///   delete-on-message path and the FK cascade.
///
/// **Data step:** a pre-existing permanent mute (a row with `muted_until IS NULL`) becomes an
/// explicit `level = 'none'`, so a row that only carries a level override is not mistaken for
/// a mute. See [`PERMANENT_MUTE_TO_NONE_SQL`].
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The data step: permanent mutes become an explicit `none` level. Public so the `rim_infra_db`
/// db-tests can reference it.
pub const PERMANENT_MUTE_TO_NONE_SQL: &str =
    "UPDATE notification_preference SET level = 'none' WHERE muted_until IS NULL";

const UP_SQL: &[&str] = &[
    "CREATE TYPE notification_level AS ENUM ('all', 'mentions', 'none')",
    "CREATE TYPE digest_frequency AS ENUM ('off', 'hourly', 'daily')",
    "CREATE TYPE notification_reason AS ENUM \
     ('direct', 'mention', 'keyword', 'all', 'thread_reply')",
    "ALTER TABLE notification_preference ADD COLUMN level notification_level NULL",
    PERMANENT_MUTE_TO_NONE_SQL,
    r#"CREATE TABLE user_notification_settings (
        user_id uuid NOT NULL,
        server_id uuid NOT NULL,
        default_level notification_level NOT NULL DEFAULT 'all',
        keywords text[] NOT NULL DEFAULT '{}',
        dnd_start time NULL,
        dnd_end time NULL,
        dnd_timezone text NULL,
        dnd_paused boolean NOT NULL DEFAULT false,
        dnd_paused_until timestamptz NULL,
        digest digest_frequency NOT NULL DEFAULT 'off',
        updated_at timestamptz NOT NULL DEFAULT now(),
        CONSTRAINT pk_user_notification_settings PRIMARY KEY (user_id, server_id),
        CONSTRAINT fk_user_notification_settings_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_user_notification_settings_server FOREIGN KEY (server_id)
            REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT user_notification_settings_dnd_check CHECK (
            (dnd_start IS NULL) = (dnd_end IS NULL)
            AND (dnd_start IS NULL) = (dnd_timezone IS NULL)
        )
    )"#,
    r#"CREATE TABLE user_notification (
        id uuid PRIMARY KEY,
        user_id uuid NOT NULL,
        server_id uuid NOT NULL,
        conversation_id uuid NOT NULL,
        message_id uuid NOT NULL,
        reason notification_reason NOT NULL,
        created_at timestamptz NOT NULL DEFAULT now(),
        read_at timestamptz NULL,
        emailed_at timestamptz NULL,
        CONSTRAINT fk_user_notification_user FOREIGN KEY (user_id)
            REFERENCES "user" (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_user_notification_server FOREIGN KEY (server_id)
            REFERENCES server (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_user_notification_conversation FOREIGN KEY (conversation_id)
            REFERENCES conversation (id) ON DELETE CASCADE ON UPDATE CASCADE,
        CONSTRAINT fk_user_notification_message FOREIGN KEY (message_id)
            REFERENCES message (id) ON DELETE CASCADE ON UPDATE CASCADE
    )"#,
    "CREATE UNIQUE INDEX idx_user_notification_user_message_unique \
     ON user_notification (user_id, message_id)",
    "CREATE INDEX idx_user_notification_user_read_created \
     ON user_notification (user_id, read_at, created_at DESC)",
    "CREATE INDEX idx_user_notification_message ON user_notification (message_id)",
];

/// Drops the new tables, the level column and the enums. The level overrides are lost; a
/// `none` level without `muted_until` reads as a permanent mute again on the old schema.
const DOWN_SQL: &[&str] = &[
    "DROP TABLE user_notification",
    "DROP TABLE user_notification_settings",
    "DELETE FROM notification_preference WHERE level IS NOT NULL AND level <> 'none' \
     AND muted_until IS NULL",
    "ALTER TABLE notification_preference DROP COLUMN level",
    "DROP TYPE notification_reason",
    "DROP TYPE digest_frequency",
    "DROP TYPE notification_level",
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
