use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::relay::model::{
    CachedIdentity, Contact, DmRequest, DmRequestId, DmRequestStatus, DroppedSpace, NewDmRequest,
    NewRelayEnvelope, NewRelayItem, RelayItemRecord, RelayQueueStats,
};
use rim_core::relay::repository::RelayRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

#[derive(FromQueryResult)]
struct IdRow {
    id: i64,
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

#[derive(FromQueryResult)]
struct QueueStatsRow {
    queued_items: i64,
    queued_bytes: i64,
    held_items: i64,
    held_bytes: i64,
}

#[derive(FromQueryResult)]
struct TimeRow {
    at: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct FlagRow {
    flag: bool,
}

#[derive(FromQueryResult)]
struct TextRow {
    value: String,
}

const ITEM_COLUMNS: &str = "i.id, e.host_server_id, e.workspace_id, e.payload, e.signature";

#[derive(FromQueryResult)]
struct ItemRow {
    id: i64,
    host_server_id: String,
    workspace_id: Uuid,
    payload: String,
    signature: String,
}

impl From<ItemRow> for RelayItemRecord {
    fn from(row: ItemRow) -> Self {
        Self {
            id: row.id,
            host_server_id: row.host_server_id,
            workspace_id: row.workspace_id,
            payload: row.payload,
            signature: row.signature,
        }
    }
}

#[derive(FromQueryResult)]
struct DroppedRow {
    removed_items: i64,
    host_server_id: String,
    workspace_id: Uuid,
}

/// Rows filed as requests, leaving out conversations decided as they arrived, whose
/// `decided_at` is their `created_at`: accepted at once (from contacts), or refused for a local
/// member and kept as ignored.
const FILED: &str = "decided_at IS DISTINCT FROM created_at";

const REQUEST_COLUMNS: &str = "id, user_id, from_principal_id, card_payload, card_signature, \
     host_server_id, conversation_id, status, created_at, decided_at, expires_at";

#[derive(FromQueryResult)]
struct RequestRow {
    id: Uuid,
    user_id: Uuid,
    from_principal_id: String,
    card_payload: String,
    card_signature: String,
    host_server_id: String,
    conversation_id: Uuid,
    status: String,
    created_at: DateTimeWithTimeZone,
    decided_at: Option<DateTimeWithTimeZone>,
    expires_at: DateTimeWithTimeZone,
}

impl TryFrom<RequestRow> for DmRequest {
    type Error = RepoError;

    fn try_from(row: RequestRow) -> RepoResult<Self> {
        Ok(Self {
            id: row.id.into(),
            user_id: row.user_id.into(),
            from_principal_id: row.from_principal_id,
            card_payload: row.card_payload,
            card_signature: row.card_signature,
            host_server_id: row.host_server_id,
            conversation_id: row.conversation_id,
            status: DmRequestStatus::parse(&row.status)
                .ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData))?,
            created_at: row.created_at.into(),
            decided_at: row.decided_at.map(Into::into),
            expires_at: row.expires_at.into(),
        })
    }
}

#[derive(FromQueryResult)]
struct ContactRow {
    principal_id: String,
    relay_urls: Vec<String>,
    added_at: DateTimeWithTimeZone,
}

impl From<ContactRow> for Contact {
    fn from(row: ContactRow) -> Self {
        Self {
            principal_id: row.principal_id,
            relay_urls: row.relay_urls,
            added_at: row.added_at.into(),
        }
    }
}

#[derive(FromQueryResult)]
struct HolderRow {
    user_id: Uuid,
    principal_id: String,
}

/// SQL that is true when the conversation `conversation_id` stays out of the user `user_id`'s
/// view: their request for it is pending or ignored, or they blocked the creator of a
/// direct-space conversation. Both arguments are SQL expressions (a placeholder or a column).
pub fn hidden(user_id: &str, conversation_id: &str) -> String {
    format!(
        r#"(EXISTS (SELECT 1 FROM relay_dm_request r
        WHERE r.user_id = {user_id} AND r.conversation_id = {conversation_id}
          AND r.status <> 'accepted')
    OR EXISTS (SELECT 1 FROM relay_block b
        JOIN "user" cu ON cu.principal_id = b.principal_id
        JOIN conversation hc ON hc.id = {conversation_id} AND hc.created_by = cu.id
        JOIN server s ON s.id = hc.server_id AND s.kind = 'direct'
        WHERE b.user_id = {user_id}))"#
    )
}

#[derive(FromQueryResult)]
struct IdentityRow {
    principal_id: String,
    served_by: String,
    identity: String,
    fetched_at: DateTimeWithTimeZone,
}

/// Relay queues, the request inbox, contacts, blocks and fetched identities, in raw SQL.
pub struct SeaOrmRelayRepository {
    db: DbHandle,
}

impl SeaOrmRelayRepository {
    /// A repository on `db`, a pool connection or a transaction.
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn all<T: FromQueryResult>(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<T>> {
        T::find_by_statement(statement(sql, values))
            .all(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn one<T: FromQueryResult>(
        &self,
        sql: &str,
        values: Vec<Value>,
    ) -> RepoResult<Option<T>> {
        T::find_by_statement(statement(sql, values))
            .one(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        self.db
            .execute_raw(statement(sql, values))
            .await
            .map(|res| res.rows_affected())
            .map_err(map_db_err)
    }

    async fn count(&self, sql: &str, values: Vec<Value>) -> RepoResult<i64> {
        Ok(self
            .one::<CountRow>(sql, values)
            .await?
            .map(|r| r.n)
            .unwrap_or(0))
    }

    async fn flag(&self, sql: &str, values: Vec<Value>) -> RepoResult<bool> {
        Ok(self
            .one::<FlagRow>(sql, values)
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn requests(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<DmRequest>> {
        self.all::<RequestRow>(sql, values)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    async fn request(&self, sql: &str, values: Vec<Value>) -> RepoResult<Option<DmRequest>> {
        self.one::<RequestRow>(sql, values)
            .await?
            .map(TryInto::try_into)
            .transpose()
    }
}

fn uuid<T: Into<Uuid>>(id: T) -> Value {
    let id: Uuid = id.into();
    id.into()
}

#[async_trait]
impl RelayRepository for SeaOrmRelayRepository {
    async fn received_through(&self, peer_server_id: &str) -> RepoResult<i64> {
        self.count(
            "SELECT received_through AS n FROM s2s_peer_inbound WHERE peer_server_id = $1",
            vec![peer_server_id.into()],
        )
        .await
    }

    async fn advance_received_through(&self, peer_server_id: &str, through: i64) -> RepoResult<()> {
        self.execute(
            "INSERT INTO s2s_peer_inbound (peer_server_id, received_through) VALUES ($1, $2) \
             ON CONFLICT (peer_server_id) DO UPDATE SET received_through = \
                 GREATEST(s2s_peer_inbound.received_through, EXCLUDED.received_through)",
            vec![peer_server_id.into(), through.into()],
        )
        .await?;
        Ok(())
    }

    async fn inbound_peers(&self) -> RepoResult<Vec<String>> {
        Ok(self
            .all::<TextRow>(
                "SELECT peer_server_id AS value FROM s2s_peer_inbound ORDER BY peer_server_id",
                vec![],
            )
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    }

    async fn insert_envelope(&self, envelope: NewRelayEnvelope) -> RepoResult<i64> {
        let values = vec![
            envelope.host_server_id.into(),
            envelope.workspace_id.into(),
            envelope.stream.into(),
            envelope.seq.into(),
            envelope.payload.into(),
            envelope.signature.into(),
        ];
        let sql = if envelope.seq.is_some() {
            "INSERT INTO relay_envelope (host_server_id, workspace_id, stream, seq, payload, signature) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (host_server_id, workspace_id, stream, seq) WHERE seq IS NOT NULL \
             DO UPDATE SET received_at = now() RETURNING id"
        } else {
            "INSERT INTO relay_envelope (host_server_id, workspace_id, stream, seq, payload, signature) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id"
        };
        self.one::<IdRow>(sql, values)
            .await?
            .map(|r| r.id)
            .ok_or_else(|| RepoError::with_message(RepoErrorKind::Internal, "no envelope id"))
    }

    async fn insert_item(&self, item: NewRelayItem) -> RepoResult<i64> {
        let device: Option<Uuid> = item.device_key_id.map(Into::into);
        let request: Option<Uuid> = item.request_id.map(Into::into);
        self.one::<IdRow>(
            "INSERT INTO relay_item (user_id, envelope_id, device_key_id, request_id, size_bytes, \
             expires_at) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            vec![
                uuid(item.user_id),
                item.envelope_id.into(),
                device.into(),
                request.into(),
                item.size_bytes.into(),
                item.expires_at.into(),
            ],
        )
        .await?
        .map(|r| r.id)
        .ok_or_else(|| RepoError::with_message(RepoErrorKind::Internal, "no item id"))
    }

    async fn queued_bytes(&self, user_id: UserId) -> RepoResult<i64> {
        self.count(
            "SELECT COALESCE(sum(size_bytes), 0)::bigint AS n FROM relay_item \
             WHERE user_id = $1 AND request_id IS NULL",
            vec![uuid(user_id)],
        )
        .await
    }

    async fn user_held_bytes(&self, user_id: UserId) -> RepoResult<i64> {
        self.count(
            "SELECT COALESCE(sum(size_bytes), 0)::bigint AS n FROM relay_item \
             WHERE user_id = $1 AND request_id IS NOT NULL",
            vec![uuid(user_id)],
        )
        .await
    }

    async fn queue_stats(&self) -> RepoResult<RelayQueueStats> {
        let row = self
            .one::<QueueStatsRow>(
                "SELECT \
                     count(*) FILTER (WHERE request_id IS NULL) AS queued_items, \
                     COALESCE(sum(size_bytes) FILTER (WHERE request_id IS NULL), 0)::bigint \
                         AS queued_bytes, \
                     count(*) FILTER (WHERE request_id IS NOT NULL) AS held_items, \
                     COALESCE(sum(size_bytes) FILTER (WHERE request_id IS NOT NULL), 0)::bigint \
                         AS held_bytes \
                 FROM relay_item",
                vec![],
            )
            .await?;
        Ok(
            row.map_or_else(RelayQueueStats::default, |r| RelayQueueStats {
                queued_items: r.queued_items,
                queued_bytes: r.queued_bytes,
                held_items: r.held_items,
                held_bytes: r.held_bytes,
            }),
        )
    }

    async fn drop_oldest(&self, user_id: UserId, bytes: i64) -> RepoResult<Vec<DroppedSpace>> {
        let rows: Vec<DroppedRow> = self
            .all(
                "WITH ranked AS (
                     SELECT id, size_bytes,
                            sum(size_bytes) OVER (ORDER BY id) AS running
                     FROM relay_item WHERE user_id = $1 AND request_id IS NULL),
                 deleted AS (
                     DELETE FROM relay_item i USING ranked r
                     WHERE i.id = r.id AND r.running - r.size_bytes < $2
                     RETURNING i.envelope_id)
                 SELECT e.host_server_id, e.workspace_id, count(*) AS removed_items
                 FROM relay_envelope e JOIN deleted d ON d.envelope_id = e.id
                 GROUP BY e.host_server_id, e.workspace_id",
                vec![uuid(user_id), bytes.into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| DroppedSpace {
                removed_items: r.removed_items.max(0) as u64,
                host_server_id: r.host_server_id,
                workspace_id: r.workspace_id,
            })
            .collect())
    }

    async fn list_items(
        &self,
        user_id: UserId,
        device_key_id: DeviceKeyId,
        after: i64,
        limit: i64,
    ) -> RepoResult<Vec<RelayItemRecord>> {
        Ok(self
            .all::<ItemRow>(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM relay_item i \
                     JOIN relay_envelope e ON e.id = i.envelope_id \
                     WHERE i.user_id = $1 AND i.request_id IS NULL \
                       AND (i.device_key_id IS NULL OR i.device_key_id = $2) AND i.id > $3 \
                     ORDER BY i.id LIMIT $4"
                ),
                vec![
                    uuid(user_id),
                    uuid(device_key_id),
                    after.into(),
                    limit.into(),
                ],
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn device_cursor(&self, device_key_id: DeviceKeyId) -> RepoResult<i64> {
        self.count(
            "SELECT COALESCE(
                 (SELECT acked_item_id FROM relay_device_cursor WHERE device_key_id = $1),
                 (SELECT max(i.id) FROM relay_item i JOIN device_key d ON d.user_id = i.user_id
                  WHERE d.id = $1 AND i.created_at < d.added_at),
                 0)::bigint AS n",
            vec![uuid(device_key_id)],
        )
        .await
    }

    async fn ack(&self, device_key_id: DeviceKeyId, up_to: i64) -> RepoResult<()> {
        self.execute(
            "INSERT INTO relay_device_cursor (device_key_id, acked_item_id) VALUES ($1, $2) \
             ON CONFLICT (device_key_id) DO UPDATE SET \
                 acked_item_id = GREATEST(relay_device_cursor.acked_item_id, EXCLUDED.acked_item_id), \
                 updated_at = now()",
            vec![uuid(device_key_id), up_to.into()],
        )
        .await?;
        Ok(())
    }

    async fn delete_acked_items(&self) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_item i
             WHERE i.request_id IS NULL AND NOT EXISTS (
                 SELECT 1 FROM device_key d
                 LEFT JOIN relay_device_cursor c ON c.device_key_id = d.id
                 WHERE d.user_id = i.user_id AND d.revoked_at IS NULL
                   AND (i.device_key_id IS NULL OR i.device_key_id = d.id)
                   AND CASE WHEN c.device_key_id IS NULL THEN i.created_at >= d.added_at
                            ELSE c.acked_item_id < i.id END)",
            vec![],
        )
        .await
    }

    async fn delete_expired_items(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_item WHERE expires_at < $1",
            vec![now.into()],
        )
        .await
    }

    async fn delete_orphan_envelopes(&self) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_envelope e \
             WHERE e.received_at < now() - interval '1 hour' \
               AND NOT EXISTS (SELECT 1 FROM relay_item i WHERE i.envelope_id = e.id)",
            vec![],
        )
        .await
    }

    async fn find_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        conversation_id: Uuid,
    ) -> RepoResult<Option<DmRequest>> {
        self.request(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM relay_dm_request \
                 WHERE user_id = $1 AND host_server_id = $2 AND conversation_id = $3"
            ),
            vec![uuid(user_id), host_server_id.into(), conversation_id.into()],
        )
        .await
    }

    async fn insert_request(&self, request: NewDmRequest) -> RepoResult<DmRequest> {
        self.request(
            &format!(
                "INSERT INTO relay_dm_request (id, user_id, from_principal_id, card_payload, \
                 card_signature, host_server_id, conversation_id, expires_at, status, \
                 created_at, decided_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), \
                         CASE WHEN $9 = 'pending' THEN NULL ELSE now() END) \
                 RETURNING {REQUEST_COLUMNS}"
            ),
            vec![
                uuid(DmRequestId::new()),
                uuid(request.user_id),
                request.from_principal_id.into(),
                request.card_payload.into(),
                request.card_signature.into(),
                request.host_server_id.into(),
                request.conversation_id.into(),
                request.expires_at.into(),
                request.status.as_str().into(),
            ],
        )
        .await?
        .ok_or_else(|| RepoError::with_message(RepoErrorKind::Internal, "no request row"))
    }

    async fn get_request(&self, user_id: UserId, id: DmRequestId) -> RepoResult<Option<DmRequest>> {
        self.request(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM relay_dm_request WHERE id = $1 AND user_id = $2"
            ),
            vec![uuid(id), uuid(user_id)],
        )
        .await
    }

    async fn list_requests(
        &self,
        user_id: UserId,
        status: Option<DmRequestStatus>,
    ) -> RepoResult<Vec<DmRequest>> {
        self.requests(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM relay_dm_request \
                 WHERE user_id = $1 AND ($2::text IS NULL OR status = $2) AND {FILED} \
                 ORDER BY created_at DESC, id DESC"
            ),
            vec![uuid(user_id), status.map(|s| s.as_str().to_string()).into()],
        )
        .await
    }

    async fn count_pending_requests(&self, user_id: UserId) -> RepoResult<i64> {
        self.count(
            "SELECT count(*) AS n FROM relay_dm_request WHERE user_id = $1 AND status = 'pending'",
            vec![uuid(user_id)],
        )
        .await
    }

    async fn count_requests_from(
        &self,
        principal_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<i64> {
        self.count(
            &format!(
                "SELECT count(*) AS n FROM relay_dm_request \
                 WHERE from_principal_id = $1 AND created_at >= $2 AND {FILED}"
            ),
            vec![principal_id.into(), since.into()],
        )
        .await
    }

    async fn oldest_request_from(
        &self,
        principal_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<Option<DateTime<Utc>>> {
        Ok(self
            .one::<TimeRow>(
                &format!(
                    "SELECT min(created_at) AS at FROM relay_dm_request \
                     WHERE from_principal_id = $1 AND created_at >= $2 AND {FILED}"
                ),
                vec![principal_id.into(), since.into()],
            )
            .await?
            .and_then(|row| row.at)
            .map(Into::into))
    }

    async fn count_requests_via_host(
        &self,
        host_server_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<i64> {
        self.count(
            &format!(
                "SELECT count(*) AS n FROM relay_dm_request \
                 WHERE host_server_id = $1 AND created_at >= $2 AND {FILED}"
            ),
            vec![host_server_id.into(), since.into()],
        )
        .await
    }

    async fn decide_request(
        &self,
        id: DmRequestId,
        status: DmRequestStatus,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<Option<DmRequest>> {
        self.request(
            &format!(
                "UPDATE relay_dm_request SET status = $2, decided_at = now(), expires_at = $3 \
                 WHERE id = $1 AND status = 'pending' AND expires_at > now() \
                 RETURNING {REQUEST_COLUMNS}"
            ),
            vec![uuid(id), status.as_str().into(), expires_at.into()],
        )
        .await
    }

    async fn release_held(&self, request_id: DmRequestId) -> RepoResult<u64> {
        self.execute(
            "UPDATE relay_item SET request_id = NULL WHERE request_id = $1",
            vec![uuid(request_id)],
        )
        .await
    }

    async fn delete_held(&self, request_id: DmRequestId) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_item WHERE request_id = $1 \
             AND id > (SELECT min(id) FROM relay_item WHERE request_id = $1)",
            vec![uuid(request_id)],
        )
        .await
    }

    async fn count_held(&self, request_id: DmRequestId) -> RepoResult<i64> {
        self.count(
            "SELECT count(*) AS n FROM relay_item WHERE request_id = $1",
            vec![uuid(request_id)],
        )
        .await
    }

    async fn request_held_bytes(&self, request_id: DmRequestId) -> RepoResult<i64> {
        self.count(
            "SELECT COALESCE(sum(size_bytes), 0)::bigint AS n FROM relay_item WHERE request_id = $1",
            vec![uuid(request_id)],
        )
        .await
    }

    async fn held_items(&self, request_id: DmRequestId) -> RepoResult<Vec<RelayItemRecord>> {
        Ok(self
            .all::<ItemRow>(
                &format!(
                    "SELECT {ITEM_COLUMNS} FROM relay_item i \
                     JOIN relay_envelope e ON e.id = i.envelope_id \
                     WHERE i.request_id = $1 ORDER BY i.id"
                ),
                vec![uuid(request_id)],
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn accepted_from(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        self.flag(
            "SELECT EXISTS (SELECT 1 FROM relay_dm_request \
             WHERE user_id = $1 AND from_principal_id = $2 AND status = 'accepted') AS flag",
            vec![uuid(user_id), principal_id.into()],
        )
        .await
    }

    async fn accepted_at_host(&self, user_id: UserId, host_server_id: &str) -> RepoResult<bool> {
        self.flag(
            "SELECT EXISTS (SELECT 1 FROM relay_dm_request \
             WHERE user_id = $1 AND host_server_id = $2 AND status = 'accepted') AS flag",
            vec![uuid(user_id), host_server_id.into()],
        )
        .await
    }

    async fn ignore_pending_from(
        &self,
        user_id: UserId,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_item i WHERE i.request_id IN (SELECT id FROM relay_dm_request \
             WHERE user_id = $1 AND from_principal_id = $2 AND status = 'pending') \
             AND i.id > (SELECT min(j.id) FROM relay_item j WHERE j.request_id = i.request_id)",
            vec![uuid(user_id), principal_id.into()],
        )
        .await?;
        self.execute(
            "UPDATE relay_dm_request SET status = 'ignored', decided_at = now(), expires_at = $3 \
             WHERE user_id = $1 AND from_principal_id = $2 AND status = 'pending'",
            vec![uuid(user_id), principal_id.into(), expires_at.into()],
        )
        .await
    }

    async fn delete_expired_requests(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM relay_dm_request WHERE expires_at < $1",
            vec![now.into()],
        )
        .await
    }

    async fn pending_holders(&self, conversation_id: Uuid) -> RepoResult<Vec<(UserId, String)>> {
        Ok(self
            .all::<HolderRow>(
                r#"SELECT r.user_id, u.principal_id FROM relay_dm_request r
                   JOIN "user" u ON u.id = r.user_id
                   WHERE r.conversation_id = $1 AND r.status = 'pending'
                     AND u.kind = 'local' AND u.principal_id IS NOT NULL"#,
                vec![conversation_id.into()],
            )
            .await?
            .into_iter()
            .map(|r| (r.user_id.into(), r.principal_id))
            .collect())
    }

    async fn request_pending(&self, user_id: UserId, conversation_id: Uuid) -> RepoResult<bool> {
        self.flag(
            "SELECT EXISTS (SELECT 1 FROM relay_dm_request \
             WHERE conversation_id = $2 AND user_id = $1 AND status = 'pending') AS flag",
            vec![uuid(user_id), conversation_id.into()],
        )
        .await
    }

    async fn conversation_hidden(
        &self,
        user_id: UserId,
        conversation_id: Uuid,
    ) -> RepoResult<bool> {
        self.flag(
            &format!("SELECT {} AS flag", hidden("$1", "$2")),
            vec![uuid(user_id), conversation_id.into()],
        )
        .await
    }

    async fn list_contacts(&self, user_id: UserId) -> RepoResult<Vec<Contact>> {
        Ok(self
            .all::<ContactRow>(
                "SELECT principal_id, relay_urls, added_at FROM relay_contact \
                 WHERE user_id = $1 ORDER BY added_at, principal_id",
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn upsert_contact(
        &self,
        user_id: UserId,
        principal_id: &str,
        relay_urls: Vec<String>,
    ) -> RepoResult<Contact> {
        self.one::<ContactRow>(
            "INSERT INTO relay_contact (user_id, principal_id, relay_urls) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, principal_id) DO UPDATE SET relay_urls = EXCLUDED.relay_urls \
             RETURNING principal_id, relay_urls, added_at",
            vec![uuid(user_id), principal_id.into(), relay_urls.into()],
        )
        .await?
        .map(Into::into)
        .ok_or_else(|| RepoError::with_message(RepoErrorKind::Internal, "no contact row"))
    }

    async fn delete_contact(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        Ok(self
            .execute(
                "DELETE FROM relay_contact WHERE user_id = $1 AND principal_id = $2",
                vec![uuid(user_id), principal_id.into()],
            )
            .await?
            == 1)
    }

    async fn is_contact(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        self.flag(
            "SELECT EXISTS (SELECT 1 FROM relay_contact \
             WHERE user_id = $1 AND principal_id = $2) AS flag",
            vec![uuid(user_id), principal_id.into()],
        )
        .await
    }

    async fn count_contacts(&self, user_id: UserId) -> RepoResult<i64> {
        self.count(
            "SELECT count(*) AS n FROM relay_contact WHERE user_id = $1",
            vec![uuid(user_id)],
        )
        .await
    }

    async fn knows(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        self.flag(
            r#"SELECT EXISTS (SELECT 1 FROM relay_contact mine
                   JOIN "user" me ON me.id = mine.user_id
                   JOIN "user" them ON them.principal_id = mine.principal_id
                   JOIN relay_contact theirs
                     ON theirs.user_id = them.id AND theirs.principal_id = me.principal_id
                   WHERE mine.user_id = $1 AND mine.principal_id = $2)
               OR EXISTS (SELECT 1 FROM relay_dm_request
                   WHERE user_id = $1 AND from_principal_id = $2 AND status = 'accepted')
               OR EXISTS (SELECT 1 FROM "user" me
                   JOIN "user" them ON them.principal_id = $2
                   JOIN relay_dm_request r ON r.user_id = them.id
                     AND r.from_principal_id = me.principal_id AND r.status = 'accepted'
                   WHERE me.id = $1) AS flag"#,
            vec![uuid(user_id), principal_id.into()],
        )
        .await
    }

    async fn block(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        Ok(self
            .execute(
                "INSERT INTO relay_block (user_id, principal_id) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING",
                vec![uuid(user_id), principal_id.into()],
            )
            .await?
            == 1)
    }

    async fn unblock(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        Ok(self
            .execute(
                "DELETE FROM relay_block WHERE user_id = $1 AND principal_id = $2",
                vec![uuid(user_id), principal_id.into()],
            )
            .await?
            == 1)
    }

    async fn is_blocked(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool> {
        self.flag(
            "SELECT EXISTS (SELECT 1 FROM relay_block \
             WHERE user_id = $1 AND principal_id = $2) AS flag",
            vec![uuid(user_id), principal_id.into()],
        )
        .await
    }

    async fn hosts_of_user(&self, user_id: UserId) -> RepoResult<Vec<String>> {
        Ok(self
            .all::<TextRow>(
                "SELECT e.host_server_id AS value FROM relay_item i \
                 JOIN relay_envelope e ON e.id = i.envelope_id WHERE i.user_id = $1 \
                 UNION SELECT host_server_id AS value FROM relay_dm_request WHERE user_id = $1 \
                 UNION SELECT host_server_id AS value FROM relay_subscription \
                 WHERE user_id = $1 AND status = 'active'",
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    }

    async fn clear_user(&self, user_id: UserId) -> RepoResult<()> {
        for sql in [
            "DELETE FROM relay_item WHERE user_id = $1",
            "DELETE FROM relay_dm_request WHERE user_id = $1",
            "DELETE FROM relay_contact WHERE user_id = $1",
            "DELETE FROM relay_block WHERE user_id = $1",
        ] {
            self.execute(sql, vec![uuid(user_id)]).await?;
        }
        Ok(())
    }

    async fn cached_identity(&self, principal_id: &str) -> RepoResult<Option<CachedIdentity>> {
        Ok(self
            .one::<IdentityRow>(
                "SELECT principal_id, served_by, identity, fetched_at FROM remote_identity_cache \
                 WHERE principal_id = $1",
                vec![principal_id.into()],
            )
            .await?
            .map(|r| CachedIdentity {
                principal_id: r.principal_id,
                served_by: r.served_by,
                identity: r.identity,
                fetched_at: r.fetched_at.into(),
            }))
    }

    async fn store_identity(
        &self,
        principal_id: &str,
        served_by: &str,
        identity: &str,
    ) -> RepoResult<()> {
        self.execute(
            "INSERT INTO remote_identity_cache (principal_id, served_by, identity, fetched_at) \
             VALUES ($1, $2, $3, now()) ON CONFLICT (principal_id) DO UPDATE SET \
                 served_by = EXCLUDED.served_by, identity = EXCLUDED.identity, \
                 fetched_at = EXCLUDED.fetched_at",
            vec![principal_id.into(), served_by.into(), identity.into()],
        )
        .await?;
        Ok(())
    }

    async fn purge_identities(&self, fetched_before: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            r#"DELETE FROM remote_identity_cache c WHERE c.fetched_at < $1
               AND NOT EXISTS (SELECT 1 FROM "user" u
                               WHERE u.principal_id = c.principal_id AND u.kind = 'remote')"#,
            vec![fetched_before.into()],
        )
        .await
    }
}
