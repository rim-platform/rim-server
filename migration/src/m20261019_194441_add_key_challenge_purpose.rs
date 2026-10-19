use sea_orm_migration::prelude::*;

/// Adds `key_challenge.purpose` and `key_challenge.relay_server_id`: what a challenge was issued
/// for, `login` (signing in, signing up, enrolling a key identity) or `join` (joining a hosted
/// workspace through the relay whose server id it holds). A challenge is consumed only for its
/// own purpose.
///
/// Existing rows, and rows a previous release writes, are `login` with no relay.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE key_challenge ADD COLUMN purpose text NOT NULL DEFAULT 'login'",
    "ALTER TABLE key_challenge ADD COLUMN relay_server_id text NULL",
    "ALTER TABLE key_challenge ADD CONSTRAINT chk_key_challenge_purpose \
     CHECK ((purpose = 'login' AND relay_server_id IS NULL) \
     OR (purpose = 'join' AND relay_server_id IS NOT NULL))",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE key_challenge DROP CONSTRAINT chk_key_challenge_purpose",
    "ALTER TABLE key_challenge DROP COLUMN relay_server_id",
    "ALTER TABLE key_challenge DROP COLUMN purpose",
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
