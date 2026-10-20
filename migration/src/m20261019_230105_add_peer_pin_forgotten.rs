use sea_orm_migration::prelude::*;

/// Adds `peer_server_forgotten` and an index on `remote_identity_cache.fetched_at`.
///
/// - `peer_server_forgotten`: the key-log head of each peer whose idle pin was deleted, so the
///   peer's next pin must extend that head. `head_payload` is the exact payload of the entry at
///   `head_seq`; each entry names the hash of the one before it, so a log holding that entry at
///   that seq holds the whole forgotten log.
/// - `idx_remote_identity_cache_fetched`: the purge of cached identities by age.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE peer_server_forgotten (
        server_id text PRIMARY KEY,
        origin text NOT NULL,
        head_seq integer NOT NULL CHECK (head_seq >= 0),
        head_payload text NOT NULL,
        forgotten_at timestamptz NOT NULL DEFAULT now()
    )"#,
    "CREATE INDEX idx_remote_identity_cache_fetched ON remote_identity_cache (fetched_at)",
];

const DOWN_SQL: &[&str] = &[
    "DROP INDEX idx_remote_identity_cache_fetched",
    "DROP TABLE peer_server_forgotten",
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
