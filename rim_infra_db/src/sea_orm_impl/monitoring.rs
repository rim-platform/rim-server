use async_trait::async_trait;
use sea_orm::{DbBackend, FromQueryResult, Statement};

use rim_core::monitoring::{DeliveryStats, MonitoringRepository, UsageStats};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};

use crate::DbHandle;
use crate::error::map_db_err;

/// Aggregate repository for local account activity and pending device delivery.
pub struct SeaOrmMonitoringRepository {
    db: DbHandle,
}

impl SeaOrmMonitoringRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn one<T: FromQueryResult>(&self, sql: &str) -> RepoResult<T> {
        T::find_by_statement(Statement::from_string(DbBackend::Postgres, sql))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| {
                RepoError::with_message(RepoErrorKind::Internal, "no monitoring aggregate")
            })
    }
}

#[derive(FromQueryResult)]
struct UsageRow {
    registered_accounts: i64,
    active_24h: i64,
    active_7d: i64,
}

#[derive(FromQueryResult)]
struct DeliveryRow {
    items: i64,
    bytes: i64,
    device_items: i64,
    users: i64,
    devices: i64,
    oldest_age_seconds: f64,
}

const USAGE_SQL: &str = r#"
    SELECT count(*) AS registered_accounts,
        count(*) FILTER (WHERE a.last_authenticated_at >= now() - interval '24 hours') AS active_24h,
        count(*) FILTER (WHERE a.last_authenticated_at >= now() - interval '7 days') AS active_7d
    FROM "user" u LEFT JOIN user_activity a ON a.user_id = u.id
    WHERE u.kind = 'local' AND u.deleted_at IS NULL
"#;

const DELIVERY_SQL: &str = r#"
    WITH pending AS (
        SELECT i.id, i.user_id, i.size_bytes, i.created_at, d.id AS device_id
        FROM relay_item i
        JOIN "user" u ON u.id = i.user_id AND u.kind = 'local' AND u.deleted_at IS NULL
        JOIN device_key d ON d.user_id = i.user_id AND d.revoked_at IS NULL
        LEFT JOIN relay_device_cursor c ON c.device_key_id = d.id
        WHERE i.request_id IS NULL AND i.expires_at > now()
          AND (i.device_key_id IS NULL OR i.device_key_id = d.id)
          AND CASE WHEN c.device_key_id IS NULL THEN i.created_at >= d.added_at
                   ELSE c.acked_item_id < i.id END
    ), recipient_items AS (
        SELECT DISTINCT id, user_id, size_bytes, created_at FROM pending
    )
    SELECT count(*) AS items, COALESCE(sum(size_bytes), 0)::bigint AS bytes,
        (SELECT count(*) FROM pending) AS device_items,
        count(DISTINCT user_id) AS users,
        (SELECT count(DISTINCT device_id) FROM pending) AS devices,
        COALESCE(GREATEST(EXTRACT(EPOCH FROM now() - min(created_at)), 0), 0)::float8 AS oldest_age_seconds
    FROM recipient_items
"#;

#[async_trait]
impl MonitoringRepository for SeaOrmMonitoringRepository {
    async fn usage_stats(&self) -> RepoResult<UsageStats> {
        let row = self.one::<UsageRow>(USAGE_SQL).await?;
        Ok(UsageStats {
            registered_accounts: row.registered_accounts,
            active_24h: row.active_24h,
            active_7d: row.active_7d,
        })
    }

    async fn delivery_stats(&self) -> RepoResult<DeliveryStats> {
        let row = self.one::<DeliveryRow>(DELIVERY_SQL).await?;
        Ok(DeliveryStats {
            items: row.items,
            bytes: row.bytes,
            device_items: row.device_items,
            users: row.users,
            devices: row.devices,
            oldest_age_seconds: row.oldest_age_seconds,
        })
    }
}
