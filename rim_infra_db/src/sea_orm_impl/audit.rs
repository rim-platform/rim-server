use async_trait::async_trait;
use entity::command_audit_log;
use rim_core::audit::repository::{
    AuditLogEntryRecord, AuditLogListQuery, AuditLogPageRecord, AuditLogRange, AuditLogRepository,
};
use rim_core::repository::error::RepoResult;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DeleteResult, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, QuerySelect, Set, Statement,
    prelude::{DateTimeWithTimeZone, Uuid},
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmAuditLogRepository {
    db: DbHandle,
}

impl SeaOrmAuditLogRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

/// Count and bounds of the rows older than a cutoff.
#[derive(FromQueryResult)]
struct RangeRow {
    n: i64,
    oldest: Option<DateTimeWithTimeZone>,
    newest: Option<DateTimeWithTimeZone>,
}

const RANGE_OLDER_THAN_SQL: &str = "SELECT count(*)::int8 AS n, min(created_at) AS oldest, \
     max(created_at) AS newest FROM command_audit_log WHERE created_at < $1";

/// Deletes one batch of the oldest rows below the cutoff, bounded by the last exported key.
const DELETE_OLDER_THAN_BATCH_SQL: &str = "DELETE FROM command_audit_log WHERE id IN (\
     SELECT id FROM command_audit_log \
     WHERE created_at < $1 AND (created_at, id) <= ($2, $3) \
     ORDER BY created_at, id LIMIT $4)";

fn to_record(r: command_audit_log::Model) -> AuditLogEntryRecord {
    AuditLogEntryRecord {
        id: r.id,
        actor_user_id: r.actor_user_id,
        server_id: r.server_id,
        command_id: r.command_id,
        initiator: r.initiator,
        args_redacted: r.args_redacted,
        sensitivity: r.sensitivity,
        dangerous: r.dangerous,
        confirmation_kind: r.confirmation_kind,
        typed_verb: r.typed_verb,
        outcome: r.outcome,
        error_kind: r.error_kind,
        capability_check_outcome: r.capability_check_outcome,
        confirmation_outcome: r.confirmation_outcome,
        parent_audit_id: r.parent_audit_id,
        created_at: r.created_at.into(),
    }
}

#[async_trait]
impl AuditLogRepository for SeaOrmAuditLogRepository {
    async fn insert(&self, entry: AuditLogEntryRecord) -> RepoResult<()> {
        let created_at: DateTimeWithTimeZone = entry.created_at.into();
        let active = command_audit_log::ActiveModel {
            id: Set(entry.id),
            actor_user_id: Set(entry.actor_user_id),
            server_id: Set(entry.server_id),
            command_id: Set(entry.command_id),
            initiator: Set(entry.initiator),
            args_redacted: Set(entry.args_redacted),
            sensitivity: Set(entry.sensitivity),
            dangerous: Set(entry.dangerous),
            confirmation_kind: Set(entry.confirmation_kind),
            typed_verb: Set(entry.typed_verb),
            outcome: Set(entry.outcome),
            error_kind: Set(entry.error_kind),
            capability_check_outcome: Set(entry.capability_check_outcome),
            confirmation_outcome: Set(entry.confirmation_outcome),
            parent_audit_id: Set(entry.parent_audit_id),
            created_at: Set(created_at),
        };
        active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn list(&self, query: AuditLogListQuery) -> RepoResult<AuditLogPageRecord> {
        let limit = query.limit.clamp(1, 200) as u64;

        let mut select = command_audit_log::Entity::find();

        if let Some(actor_id) = query.actor_user_id {
            select = select.filter(command_audit_log::Column::ActorUserId.eq::<Uuid>(actor_id));
        }
        if let Some(server_id) = query.server_id {
            select = select.filter(command_audit_log::Column::ServerId.eq::<Uuid>(server_id));
        }
        if let Some(command_id) = &query.command_id {
            select = select.filter(command_audit_log::Column::CommandId.eq(command_id.as_str()));
        }
        if let Some(typed_verb) = &query.typed_verb {
            select = select.filter(command_audit_log::Column::TypedVerb.eq(typed_verb.as_str()));
        }
        if let Some(since) = query.since {
            let since_tz: DateTimeWithTimeZone = since.into();
            select = select.filter(command_audit_log::Column::CreatedAt.gte(since_tz));
        }
        if let Some(until) = query.until {
            let until_tz: DateTimeWithTimeZone = until.into();
            select = select.filter(command_audit_log::Column::CreatedAt.lte(until_tz));
        }

        // Cursor pagination: `(created_at DESC, id DESC)`
        if let Some((cursor_at, cursor_id)) = query.cursor {
            let cursor_at_tz: DateTimeWithTimeZone = cursor_at.into();
            select = select.filter(
                sea_orm::Condition::any()
                    .add(command_audit_log::Column::CreatedAt.lt(cursor_at_tz))
                    .add(
                        sea_orm::Condition::all()
                            .add(command_audit_log::Column::CreatedAt.eq(cursor_at_tz))
                            .add(command_audit_log::Column::Id.lt(cursor_id)),
                    ),
            );
        }

        // Fetch limit+1 to detect next page.
        let rows = select
            .order_by_desc(command_audit_log::Column::CreatedAt)
            .order_by_desc(command_audit_log::Column::Id)
            .limit(limit + 1)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let has_next = rows.len() as u64 > limit;
        let rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

        let next_cursor = if has_next {
            rows.last().map(|r| {
                let dt: chrono::DateTime<chrono::Utc> = r.created_at.into();
                (dt, r.id)
            })
        } else {
            None
        };

        let items = rows.into_iter().map(to_record).collect();

        Ok(AuditLogPageRecord {
            items,
            next_cursor,
            has_next_page: has_next,
            has_prev_page: query.cursor.is_some(),
        })
    }

    async fn delete_older_than(&self, cutoff: chrono::DateTime<chrono::Utc>) -> RepoResult<u64> {
        let cutoff_tz: DateTimeWithTimeZone = cutoff.into();
        let DeleteResult { rows_affected } = command_audit_log::Entity::delete_many()
            .filter(command_audit_log::Column::CreatedAt.lt(cutoff_tz))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows_affected)
    }

    async fn range_older_than(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<AuditLogRange> {
        let cutoff_tz: DateTimeWithTimeZone = cutoff.into();
        let row = RangeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            RANGE_OLDER_THAN_SQL,
            [cutoff_tz.into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.map_or(
            AuditLogRange {
                count: 0,
                oldest: None,
                newest: None,
            },
            |r| AuditLogRange {
                count: u64::try_from(r.n).unwrap_or(0),
                oldest: r.oldest.map(Into::into),
                newest: r.newest.map(Into::into),
            },
        ))
    }

    async fn list_older_than(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
        after: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
        limit: u32,
    ) -> RepoResult<Vec<AuditLogEntryRecord>> {
        let cutoff_tz: DateTimeWithTimeZone = cutoff.into();
        let mut select = command_audit_log::Entity::find()
            .filter(command_audit_log::Column::CreatedAt.lt(cutoff_tz));
        if let Some((after_at, after_id)) = after {
            let after_tz: DateTimeWithTimeZone = after_at.into();
            select = select.filter(
                sea_orm::Condition::any()
                    .add(command_audit_log::Column::CreatedAt.gt(after_tz))
                    .add(
                        sea_orm::Condition::all()
                            .add(command_audit_log::Column::CreatedAt.eq(after_tz))
                            .add(command_audit_log::Column::Id.gt(after_id)),
                    ),
            );
        }
        let rows = select
            .order_by_asc(command_audit_log::Column::CreatedAt)
            .order_by_asc(command_audit_log::Column::Id)
            .limit(u64::from(limit.max(1)))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(rows.into_iter().map(to_record).collect())
    }

    async fn delete_older_than_batch(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
        up_to: (chrono::DateTime<chrono::Utc>, Uuid),
        limit: u32,
    ) -> RepoResult<u64> {
        let cutoff_tz: DateTimeWithTimeZone = cutoff.into();
        let up_to_tz: DateTimeWithTimeZone = up_to.0.into();
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                DELETE_OLDER_THAN_BATCH_SQL,
                [
                    cutoff_tz.into(),
                    up_to_tz.into(),
                    up_to.1.into(),
                    i64::from(limit.max(1)).into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected())
    }
}
