use sea_orm_migration::prelude::*;

/// Adds a unique index on `relay_abuse_report (reporter_user_id, request_id)` for reports of a
/// DM request, so one account reports one request once.
///
/// Before creating the index it deletes the reports that repeat an earlier one of the same
/// account and request, keeping the oldest of each pair.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "DELETE FROM relay_abuse_report r USING relay_abuse_report k \
     WHERE r.request_id IS NOT NULL AND r.reporter_user_id IS NOT NULL \
       AND k.request_id = r.request_id AND k.reporter_user_id = r.reporter_user_id \
       AND (k.created_at, k.id) < (r.created_at, r.id)",
    "CREATE UNIQUE INDEX idx_relay_abuse_report_reporter_request \
     ON relay_abuse_report (reporter_user_id, request_id) WHERE request_id IS NOT NULL",
];

const DOWN_SQL: &str = "DROP INDEX IF EXISTS idx_relay_abuse_report_reporter_request";

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
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
