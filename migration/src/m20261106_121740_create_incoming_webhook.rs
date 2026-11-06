use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// `incoming_webhook`: a URL that posts to one channel as one bot, stored as the hash of its token.
///
/// At most one row per channel is not revoked. `created_by` is a plain reference: account deletion
/// keeps the row as a tombstone. The previous release's server never reads the new table.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE incoming_webhook (
    id uuid PRIMARY KEY,
    server_id uuid NOT NULL REFERENCES server (id) ON DELETE CASCADE,
    conversation_id uuid NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    bot_user_id uuid NOT NULL REFERENCES bot (user_id) ON DELETE CASCADE,
    preset text NOT NULL CHECK (preset IN ('plain', 'github')),
    token_hash text NOT NULL UNIQUE,
    created_by uuid NOT NULL REFERENCES "user" (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NULL,
    revoked_at timestamptz NULL
)"#,
    "CREATE UNIQUE INDEX idx_incoming_webhook_active_conversation \
     ON incoming_webhook (conversation_id) WHERE revoked_at IS NULL",
    "CREATE INDEX idx_incoming_webhook_server_id ON incoming_webhook (server_id)",
];

const DOWN_SQL: &[&str] = &["DROP TABLE incoming_webhook"];

async fn run(manager: &SchemaManager<'_>, statements: &[&str]) -> Result<(), DbErr> {
    let db = manager.get_connection();
    for sql in statements {
        db.execute_unprepared(sql).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, UP_SQL).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        run(manager, DOWN_SQL).await
    }
}
