use sea_orm_migration::prelude::*;

/// Adds `s2s_peer_delivery.failing_since`: when the current run of failed deliveries to a relay
/// began, set by the first failure after a success and cleared by the next success. A relay
/// failing for longer than the configured period is dead: the outbox queues nothing new for it.
///
/// Existing rows, and rows a previous release writes, have none; a failing relay gets one at
/// its next failure.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] =
    &["ALTER TABLE s2s_peer_delivery ADD COLUMN failing_since timestamptz NULL"];

const DOWN_SQL: &[&str] = &["ALTER TABLE s2s_peer_delivery DROP COLUMN failing_since"];

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
