use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::relay::model::{
    AbuseReport, AbuseReportFilter, AbuseReportId, NewAbuseReport, PrincipalBlock,
};
use rim_core::relay::repository::RelayAbuseRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

#[derive(FromQueryResult)]
struct FlagRow {
    flag: bool,
}

#[derive(FromQueryResult)]
struct BlockRow {
    principal_id: String,
    reason: String,
    created_at: DateTimeWithTimeZone,
}

const REPORT_COLUMNS: &str = "id, reporter_user_id, subject_principal_id, subject_server_id, \
     request_id, reason, created_at, resolved_at";

#[derive(FromQueryResult)]
struct ReportRow {
    id: Uuid,
    reporter_user_id: Option<Uuid>,
    subject_principal_id: Option<String>,
    subject_server_id: Option<String>,
    request_id: Option<Uuid>,
    reason: String,
    created_at: DateTimeWithTimeZone,
    resolved_at: Option<DateTimeWithTimeZone>,
}

impl From<ReportRow> for AbuseReport {
    fn from(row: ReportRow) -> Self {
        Self {
            id: row.id.into(),
            reporter_user_id: row.reporter_user_id.map(Into::into),
            subject_principal_id: row.subject_principal_id,
            subject_server_id: row.subject_server_id,
            request_id: row.request_id.map(Into::into),
            reason: row.reason,
            created_at: row.created_at.into(),
            resolved_at: row.resolved_at.map(Into::into),
        }
    }
}

/// Principal blocks and abuse reports on `relay_principal_block` and `relay_abuse_report`.
pub struct SeaOrmRelayAbuseRepository {
    db: DbHandle,
}

impl SeaOrmRelayAbuseRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        self.db
            .execute_raw(statement(sql, values))
            .await
            .map(|res| res.rows_affected())
            .map_err(map_db_err)
    }

    async fn report(&self, sql: &str, values: Vec<Value>) -> RepoResult<Option<AbuseReport>> {
        Ok(ReportRow::find_by_statement(statement(sql, values))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(Into::into))
    }
}

#[async_trait]
impl RelayAbuseRepository for SeaOrmRelayAbuseRepository {
    async fn block_principal(&self, principal_id: &str, reason: &str) -> RepoResult<bool> {
        let row = FlagRow::find_by_statement(statement(
            "INSERT INTO relay_principal_block (principal_id, reason) VALUES ($1, $2) \
             ON CONFLICT (principal_id) DO UPDATE SET reason = EXCLUDED.reason \
             RETURNING (xmax = 0) AS flag",
            vec![principal_id.into(), reason.into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some_and(|r| r.flag))
    }

    async fn unblock_principal(&self, principal_id: &str) -> RepoResult<bool> {
        Ok(self
            .execute(
                "DELETE FROM relay_principal_block WHERE principal_id = $1",
                vec![principal_id.into()],
            )
            .await?
            == 1)
    }

    async fn is_principal_blocked(&self, principal_id: &str) -> RepoResult<bool> {
        let row = FlagRow::find_by_statement(statement(
            "SELECT EXISTS (SELECT 1 FROM relay_principal_block WHERE principal_id = $1) AS flag",
            vec![principal_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some_and(|r| r.flag))
    }

    async fn list_principal_blocks(&self) -> RepoResult<Vec<PrincipalBlock>> {
        Ok(BlockRow::find_by_statement(statement(
            "SELECT principal_id, reason, created_at FROM relay_principal_block \
             ORDER BY created_at DESC, principal_id",
            vec![],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?
        .into_iter()
        .map(|r| PrincipalBlock {
            principal_id: r.principal_id,
            reason: r.reason,
            created_at: r.created_at.into(),
        })
        .collect())
    }

    async fn ignore_requests_from(
        &self,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_item i WHERE i.request_id IN (SELECT id FROM relay_dm_request \
             WHERE from_principal_id = $1 AND status = 'pending') \
             AND i.id > (SELECT min(j.id) FROM relay_item j WHERE j.request_id = i.request_id)",
            vec![principal_id.into()],
        )
        .await?;
        self.execute(
            "UPDATE relay_dm_request SET status = 'ignored', decided_at = now(), expires_at = $2 \
             WHERE from_principal_id = $1 AND status = 'pending'",
            vec![principal_id.into(), expires_at.into()],
        )
        .await
    }

    async fn clear_requests_ignored_by_block(
        &self,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.execute(
            "UPDATE relay_dm_request r SET status = 'pending', created_at = now(), \
             decided_at = now(), expires_at = $2 \
             FROM relay_principal_block pb \
             WHERE pb.principal_id = $1 AND r.from_principal_id = $1 AND r.status = 'ignored' \
               AND r.decided_at >= pb.created_at \
               AND EXISTS (SELECT 1 FROM relay_item i WHERE i.request_id = r.id) \
               AND NOT EXISTS (SELECT 1 FROM relay_block b \
                               WHERE b.user_id = r.user_id AND b.principal_id = $1)",
            vec![principal_id.into(), expires_at.into()],
        )
        .await
    }

    async fn insert_report(&self, report: NewAbuseReport) -> RepoResult<AbuseReport> {
        let reporter: Option<Uuid> = report.reporter_user_id.map(Into::into);
        let request: Option<Uuid> = report.request_id.map(Into::into);
        let id: Uuid = AbuseReportId::new().into();
        self.report(
            &format!(
                "INSERT INTO relay_abuse_report (id, reporter_user_id, subject_principal_id, \
                 subject_server_id, request_id, reason) VALUES ($1, $2, $3, $4, $5, $6) \
                 RETURNING {REPORT_COLUMNS}"
            ),
            vec![
                id.into(),
                reporter.into(),
                report.subject_principal_id.into(),
                report.subject_server_id.into(),
                request.into(),
                report.reason.into(),
            ],
        )
        .await?
        .ok_or_else(|| RepoError::with_message(RepoErrorKind::Internal, "no report row"))
    }

    async fn list_reports(&self, filter: AbuseReportFilter) -> RepoResult<Vec<AbuseReport>> {
        let after: Option<Uuid> = filter.after.map(Into::into);
        let limit = i64::try_from(filter.limit).unwrap_or(i64::MAX);
        Ok(ReportRow::find_by_statement(statement(
            &format!(
                "SELECT {REPORT_COLUMNS} FROM relay_abuse_report \
                 WHERE (NOT $1 OR resolved_at IS NULL) \
                   AND ($2::text IS NULL OR subject_principal_id = $2) \
                   AND ($3::uuid IS NULL OR (created_at, id) > \
                        (SELECT created_at, id FROM relay_abuse_report WHERE id = $3)) \
                 ORDER BY created_at, id LIMIT $4"
            ),
            vec![
                filter.open_only.into(),
                filter.subject_principal_id.into(),
                after.into(),
                limit.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?
        .into_iter()
        .map(Into::into)
        .collect())
    }

    async fn resolve_report(&self, id: AbuseReportId) -> RepoResult<Option<AbuseReport>> {
        let id: Uuid = id.into();
        self.report(
            &format!(
                "UPDATE relay_abuse_report SET resolved_at = COALESCE(resolved_at, now()) \
                 WHERE id = $1 RETURNING {REPORT_COLUMNS}"
            ),
            vec![id.into()],
        )
        .await
    }
}
