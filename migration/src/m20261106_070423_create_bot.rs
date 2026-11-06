use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// Bot accounts.
///
/// - `user.kind` also takes `bot`, and a bot's row is live without a principal id: it has no key,
///   so it cannot sign in.
/// - `bot`: a bot's workspace, who runs it, where what it reads goes and who added it.
/// - `bot_token`: its tokens, stored as hashes.
///
/// `created_by` is a plain reference: account deletion keeps the row as a tombstone, so what a
/// deleted account added still names it. The previous release's server never reads the new
/// tables and writes no bot.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_kind"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_kind CHECK (kind IN ('local', 'remote', 'bot'))"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal
       CHECK (principal_id IS NOT NULL OR deleted_at IS NOT NULL OR kind = 'bot')"#,
    r#"CREATE TABLE bot (
    user_id uuid PRIMARY KEY REFERENCES "user" (id) ON DELETE CASCADE,
    server_id uuid NOT NULL REFERENCES server (id) ON DELETE CASCADE,
    run_by text NOT NULL,
    reads_go_to text NOT NULL,
    created_by uuid NOT NULL REFERENCES "user" (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    deactivated_at timestamptz NULL
)"#,
    "CREATE INDEX idx_bot_server_id ON bot (server_id)",
    r#"CREATE TABLE bot_token (
    id uuid PRIMARY KEY,
    bot_user_id uuid NOT NULL REFERENCES bot (user_id) ON DELETE CASCADE,
    label text NOT NULL,
    token_hash text NOT NULL UNIQUE,
    created_by uuid NOT NULL REFERENCES "user" (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NULL,
    revoked_at timestamptz NULL
)"#,
    "CREATE INDEX idx_bot_token_bot_user_id ON bot_token (bot_user_id)",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE bot_token",
    "DROP TABLE bot",
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal
       CHECK (principal_id IS NOT NULL OR deleted_at IS NOT NULL)"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_kind"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_kind CHECK (kind IN ('local', 'remote'))"#,
];

/// What must be gone before `down` may run: the `user` rows of bots, which the restored
/// constraints would refuse.
const DOWN_GUARD_SQL: &str = r#"SELECT count(*) AS n FROM "user" WHERE kind = 'bot'"#;

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
                "bot accounts exist; remove them before reverting create_bot".to_string(),
            ));
        }
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
