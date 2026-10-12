use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::ip_block;
use rim_core::ip_block::model::{IpBlock, IpBlockId, NewIpBlock};
use rim_core::ip_block::repository::IpBlockRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{
    ColumnTrait, Condition, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;

/// The IP deny list on `ip_block`.
pub struct SeaOrmIpBlockRepository {
    db: DbHandle,
}

impl SeaOrmIpBlockRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[derive(FromQueryResult)]
struct UpsertRow {
    id: Uuid,
    cidr: String,
    reason: String,
    expires_at: Option<DateTimeWithTimeZone>,
    created_at: DateTimeWithTimeZone,
    inserted: bool,
}

#[async_trait]
impl IpBlockRepository for SeaOrmIpBlockRepository {
    async fn upsert(&self, block: NewIpBlock) -> RepoResult<(IpBlock, bool)> {
        let id: Uuid = IpBlockId::new().into();
        let row = UpsertRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO ip_block (id, cidr, reason, expires_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (cidr) DO UPDATE SET reason = EXCLUDED.reason, \
             expires_at = EXCLUDED.expires_at \
             RETURNING id, cidr, reason, expires_at, created_at, (xmax = 0) AS inserted",
            vec![
                id.into(),
                block.cidr.into(),
                block.reason.into(),
                block.expires_at.into(),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?
        .ok_or_else(|| {
            RepoError::with_message(RepoErrorKind::Internal, "the upsert returned no row")
        })?;
        let inserted = row.inserted;
        Ok((
            IpBlock {
                id: row.id.into(),
                cidr: row.cidr,
                reason: row.reason,
                expires_at: row.expires_at.map(Into::into),
                created_at: row.created_at.into(),
            },
            inserted,
        ))
    }

    async fn delete(&self, cidr: &str) -> RepoResult<bool> {
        let result = ip_block::Entity::delete_many()
            .filter(ip_block::Column::Cidr.eq(cidr))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn list(&self) -> RepoResult<Vec<IpBlock>> {
        let models = ip_block::Entity::find()
            .order_by_asc(ip_block::Column::CreatedAt)
            .order_by_asc(ip_block::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn list_active(&self, now: DateTime<Utc>) -> RepoResult<Vec<IpBlock>> {
        let models = ip_block::Entity::find()
            .filter(
                Condition::any()
                    .add(ip_block::Column::ExpiresAt.is_null())
                    .add(ip_block::Column::ExpiresAt.gt(now)),
            )
            .order_by_asc(ip_block::Column::CreatedAt)
            .order_by_asc(ip_block::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn delete_expired(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        let result = ip_block::Entity::delete_many()
            .filter(ip_block::Column::ExpiresAt.lte(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected)
    }
}
