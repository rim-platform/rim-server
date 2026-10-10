use sea_orm_migration::prelude::*;

/// Drops indexes no query needs and the plugin columns of `capability_registry`.
///
/// - Seven single-column indexes that are the leading column of a wider index on the same
///   table, which serves the same lookups.
/// - `idx_command_audit_log_parent_audit_id` and `idx_plugin_bundle_created_at`, which no query
///   filters on.
/// - `capability_registry.plugin_id`, its index and the two CHECKs that allowed plugin rows, plus
///   the `(source_kind, retired_at)` index. Every registry row is a builtin, so the source-kind
///   CHECK now allows only `'builtin'`.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "DROP INDEX IF EXISTS idx_read_state_user",
    "DROP INDEX IF EXISTS idx_custom_emoji_server",
    "DROP INDEX IF EXISTS idx_keybind_scope_subject",
    "DROP INDEX IF EXISTS idx_link_preview_message",
    "DROP INDEX IF EXISTS idx_macro_server_id",
    "DROP INDEX IF EXISTS idx_message_reaction_message_id",
    r#"DROP INDEX IF EXISTS "idx-user_command-owner_user_id""#,
    "DROP INDEX IF EXISTS idx_command_audit_log_parent_audit_id",
    "DROP INDEX IF EXISTS idx_plugin_bundle_created_at",
    "DROP INDEX IF EXISTS idx_capability_registry_plugin_id",
    "DROP INDEX IF EXISTS idx_capability_registry_source_kind_retired_at",
    "ALTER TABLE capability_registry \
     DROP CONSTRAINT IF EXISTS capability_registry_plugin_id_consistency_check",
    "ALTER TABLE capability_registry DROP CONSTRAINT IF EXISTS capability_registry_source_kind_check",
    "ALTER TABLE capability_registry DROP COLUMN IF EXISTS plugin_id",
    "ALTER TABLE capability_registry ADD CONSTRAINT capability_registry_source_kind_check \
     CHECK (source_kind = 'builtin')",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE capability_registry DROP CONSTRAINT IF EXISTS capability_registry_source_kind_check",
    "ALTER TABLE capability_registry ADD COLUMN IF NOT EXISTS plugin_id uuid NULL",
    "ALTER TABLE capability_registry ADD CONSTRAINT capability_registry_source_kind_check \
     CHECK (source_kind IN ('builtin', 'plugin'))",
    "ALTER TABLE capability_registry ADD CONSTRAINT capability_registry_plugin_id_consistency_check \
     CHECK ((source_kind = 'builtin' AND plugin_id IS NULL) OR \
            (source_kind = 'plugin'  AND plugin_id IS NOT NULL))",
    "CREATE INDEX IF NOT EXISTS idx_capability_registry_source_kind_retired_at \
     ON capability_registry (source_kind, retired_at)",
    "CREATE INDEX IF NOT EXISTS idx_capability_registry_plugin_id \
     ON capability_registry (plugin_id)",
    "CREATE INDEX IF NOT EXISTS idx_plugin_bundle_created_at ON plugin_bundle (created_at)",
    "CREATE INDEX IF NOT EXISTS idx_command_audit_log_parent_audit_id \
     ON command_audit_log (parent_audit_id) WHERE parent_audit_id IS NOT NULL",
    r#"CREATE INDEX IF NOT EXISTS "idx-user_command-owner_user_id" ON user_command (owner_user_id)"#,
    "CREATE INDEX IF NOT EXISTS idx_message_reaction_message_id ON message_reaction (message_id)",
    "CREATE INDEX IF NOT EXISTS idx_macro_server_id ON macro (server_id)",
    "CREATE INDEX IF NOT EXISTS idx_link_preview_message ON link_preview (message_id)",
    "CREATE INDEX IF NOT EXISTS idx_keybind_scope_subject ON keybind (scope, subject_id)",
    "CREATE INDEX IF NOT EXISTS idx_custom_emoji_server ON custom_emoji (server_id)",
    "CREATE INDEX IF NOT EXISTS idx_read_state_user ON conversation_read_state (user_id)",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in UP_SQL {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    /// Recreates the dropped indexes, the `plugin_id` column and the original CHECKs.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in DOWN_SQL {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
