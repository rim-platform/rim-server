use async_trait::async_trait;
use chrono::Duration;
use entity::device_link;
use rim_core::device_link::model::{
    DeviceLink, DeviceLinkId, DeviceLinkState, NewDeviceLink, SealedBytes,
};
use rim_core::device_link::repository::DeviceLinkRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, QueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;
use crate::sea_orm_impl::user_lock::lock_user_row;

/// Every column a read or transition returns. `expired` is computed by the database clock, the
/// same clock the transitions compare against.
const LINK_COLUMNS: &str = "id, link_hash, user_id, state, offer_nonce, offer_ciphertext, \
     answer_nonce, answer_ciphertext, created_at, expires_at, offered_at, answered_at, \
     ended_at, expires_at <= now() AS expired";

/// `consumed` and `cancelled` keep no sealed bytes.
const END_LINK: &str = "ended_at = now(), offer_nonce = NULL, offer_ciphertext = NULL, \
     answer_nonce = NULL, answer_ciphertext = NULL";

/// `DeviceLinkState::LIVE` as an SQL list: `('open', 'offered', 'answered')`.
fn live_states() -> String {
    let states: Vec<String> = DeviceLinkState::LIVE
        .iter()
        .map(|s| format!("'{}'", s.as_str()))
        .collect();
    format!("({})", states.join(", "))
}

/// Seconds as `make_interval` takes them.
fn seconds(duration: Duration) -> Value {
    (duration.num_milliseconds() as f64 / 1000.0).into()
}

fn sealed(nonce: Option<Vec<u8>>, ciphertext: Option<Vec<u8>>) -> Option<SealedBytes> {
    match (nonce, ciphertext) {
        (Some(nonce), Some(ciphertext)) => Some(SealedBytes { nonce, ciphertext }),
        _ => None,
    }
}

fn to_link(row: &QueryResult) -> RepoResult<DeviceLink> {
    let m = device_link::Model::from_query_result(row, "").map_err(map_db_err)?;
    let expired: bool = row.try_get("", "expired").map_err(map_db_err)?;
    let state = DeviceLinkState::parse(&m.state)
        .ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData))?;
    // Expired rows are never handed out with their sealed bytes.
    let (offer, answer) = if expired {
        (None, None)
    } else {
        (
            sealed(m.offer_nonce, m.offer_ciphertext),
            sealed(m.answer_nonce, m.answer_ciphertext),
        )
    };
    Ok(DeviceLink {
        id: DeviceLinkId::from(m.id),
        user_id: m.user_id.into(),
        state,
        expired,
        offer,
        answer,
        created_at: m.created_at.into(),
        expires_at: m.expires_at.into(),
        offered_at: m.offered_at.map(Into::into),
        answered_at: m.answered_at.map(Into::into),
        ended_at: m.ended_at.map(Into::into),
    })
}

pub struct SeaOrmDeviceLinkRepository {
    db: DbHandle,
}

impl SeaOrmDeviceLinkRepository {
    /// A repository on `db`, a pool connection or a transaction.
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn query_link(&self, sql: &str, values: Vec<Value>) -> RepoResult<Option<DeviceLink>> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .map_err(map_db_err)?;
        row.as_ref().map(to_link).transpose()
    }
}

#[async_trait]
impl DeviceLinkRepository for SeaOrmDeviceLinkRepository {
    async fn lock_user(&self, user_id: UserId) -> RepoResult<bool> {
        lock_user_row(&self.db, user_id).await
    }

    async fn cancel_live_links(&self, user_id: UserId) -> RepoResult<Vec<DeviceLinkId>> {
        let id: Uuid = user_id.into();
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "UPDATE device_link SET state = 'cancelled', {END_LINK} \
                     WHERE user_id = $1 AND state IN {} RETURNING id",
                    live_states()
                ),
                [id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                row.try_get::<Uuid>("", "id")
                    .map(DeviceLinkId::from)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn create(&self, link: NewDeviceLink) -> RepoResult<DeviceLink> {
        let id: Uuid = DeviceLinkId::new().into();
        let user_id: Uuid = link.user_id.into();
        self.query_link(
            &format!(
                "INSERT INTO device_link (id, link_hash, user_id, expires_at) \
                 VALUES ($1, $2, $3, now() + make_interval(secs => $4)) \
                 RETURNING {LINK_COLUMNS}"
            ),
            vec![
                id.into(),
                link.link_hash.into(),
                user_id.into(),
                seconds(link.ttl),
            ],
        )
        .await?
        .ok_or_else(|| RepoError::new(RepoErrorKind::Internal))
    }

    async fn find(&self, link_hash: &[u8]) -> RepoResult<Option<DeviceLink>> {
        self.query_link(
            &format!("SELECT {LINK_COLUMNS} FROM device_link WHERE link_hash = $1"),
            vec![link_hash.to_vec().into()],
        )
        .await
    }

    async fn offer(&self, link_hash: &[u8], offer: SealedBytes) -> RepoResult<Option<DeviceLink>> {
        self.query_link(
            &format!(
                "UPDATE device_link \
                 SET state = 'offered', offer_nonce = $2, offer_ciphertext = $3, \
                     offered_at = now() \
                 WHERE link_hash = $1 AND state = 'open' AND expires_at > now() \
                 RETURNING {LINK_COLUMNS}"
            ),
            vec![
                link_hash.to_vec().into(),
                offer.nonce.into(),
                offer.ciphertext.into(),
            ],
        )
        .await
    }

    async fn answer(
        &self,
        link_hash: &[u8],
        user_id: UserId,
        answer: SealedBytes,
        ttl: Duration,
    ) -> RepoResult<Option<DeviceLink>> {
        let user_id: Uuid = user_id.into();
        self.query_link(
            &format!(
                "UPDATE device_link \
                 SET state = 'answered', answer_nonce = $3, answer_ciphertext = $4, \
                     answered_at = now(), expires_at = now() + make_interval(secs => $5) \
                 WHERE link_hash = $1 AND user_id = $2 AND state = 'offered' \
                     AND expires_at > now() \
                 RETURNING {LINK_COLUMNS}"
            ),
            vec![
                link_hash.to_vec().into(),
                user_id.into(),
                answer.nonce.into(),
                answer.ciphertext.into(),
                seconds(ttl),
            ],
        )
        .await
    }

    async fn complete(&self, link_hash: &[u8], user_id: UserId) -> RepoResult<Option<DeviceLink>> {
        let user_id: Uuid = user_id.into();
        self.query_link(
            &format!(
                "UPDATE device_link SET state = 'consumed', {END_LINK} \
                 WHERE link_hash = $1 AND user_id = $2 AND state = 'answered' \
                     AND expires_at > now() \
                 RETURNING {LINK_COLUMNS}"
            ),
            vec![link_hash.to_vec().into(), user_id.into()],
        )
        .await
    }

    async fn cancel(
        &self,
        link_hash: &[u8],
        user_id: Option<UserId>,
    ) -> RepoResult<Option<DeviceLink>> {
        let user_id: Option<Uuid> = user_id.map(Into::into);
        self.query_link(
            &format!(
                "UPDATE device_link SET state = 'cancelled', {END_LINK} \
                 WHERE link_hash = $1 AND ($2::uuid IS NULL OR user_id = $2) \
                     AND state IN {} AND expires_at > now() \
                 RETURNING {LINK_COLUMNS}",
                live_states()
            ),
            vec![link_hash.to_vec().into(), user_id.into()],
        )
        .await
    }

    async fn delete_expired(&self, grace: Duration) -> RepoResult<u64> {
        let res = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM device_link WHERE expires_at < now() - make_interval(secs => $1)",
                [seconds(grace)],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected())
    }
}
