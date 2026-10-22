use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::host::model::{
    HomeHandle, NewRemoteUser, OutboxBacklog, OutboxRow, OutboxStats, PeerDelivery, RemoteMember,
    SeqDecision, UserKind,
};
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{
    ConnectionTrait, DbBackend, DbErr, FromQueryResult, Statement, TransactionError, Value,
};

use crate::DbHandle;
use crate::error::map_db_err;

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn uuid<T: Into<Uuid>>(id: T) -> Value {
    let id: Uuid = id.into();
    id.into()
}

fn internal(message: &str) -> RepoError {
    RepoError::with_message(RepoErrorKind::Internal, message)
}

fn parse_role(role: &str) -> RepoResult<ServerMemberRole> {
    match role {
        "owner" => Ok(ServerMemberRole::Owner),
        "moderator" => Ok(ServerMemberRole::Moderator),
        "member" => Ok(ServerMemberRole::Member),
        "guest" => Ok(ServerMemberRole::Guest),
        _ => Err(RepoError::new(RepoErrorKind::InvalidData)),
    }
}

#[derive(FromQueryResult)]
struct UuidRow {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct I64Row {
    n: i64,
}

#[derive(FromQueryResult)]
struct FlagRow {
    flag: bool,
}

#[derive(FromQueryResult)]
struct TextRow {
    value: String,
}

#[derive(FromQueryResult)]
struct OptTextRow {
    value: Option<String>,
}

#[derive(FromQueryResult)]
struct UserKindRow {
    id: Uuid,
    kind: String,
}

#[derive(FromQueryResult)]
struct RemoteMemberRow {
    user_id: Uuid,
    principal_id: String,
    role: String,
    home_relay: Option<String>,
    card_payload: Option<String>,
}

#[derive(FromQueryResult)]
struct OutboxDbRow {
    id: i64,
    item: String,
    size_bytes: i32,
}

#[derive(FromQueryResult)]
struct DeliveryRow {
    delivered_through: i64,
    failures: i32,
    next_attempt_at: DateTimeWithTimeZone,
    failing_since: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct BacklogRow {
    rows: i64,
    bytes: i64,
    failing_since: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct OutboxStatsRow {
    rows: i64,
    bytes: i64,
    peer_max_rows: i64,
    peer_max_bytes: i64,
    dead_peers: i64,
}

#[derive(FromQueryResult)]
struct HeadRow {
    stream: String,
    seq: i64,
}

#[derive(FromQueryResult)]
struct PredecessorRow {
    predecessor_host: Option<String>,
    predecessor_conversation_id: Option<Uuid>,
}

#[derive(FromQueryResult)]
struct HighestRow {
    request_highest_seq: Option<i64>,
}

/// The direct space, remote users, event streams, the outbox and the device request window,
/// in raw SQL.
pub struct SeaOrmHostRepository {
    db: DbHandle,
}

impl SeaOrmHostRepository {
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
}

#[async_trait]
impl HostRepository for SeaOrmHostRepository {
    async fn ensure_direct_space(&self, name: &str) -> RepoResult<ServerId> {
        self.execute(
            "INSERT INTO server (id, name, kind, created_at, updated_at) \
             VALUES ($1, $2, 'direct', now(), now()) \
             ON CONFLICT (kind) WHERE kind = 'direct' DO NOTHING",
            vec![uuid(ServerId::new()), name.into()],
        )
        .await?;
        self.direct_space()
            .await?
            .ok_or_else(|| internal("the direct space is missing after its insert"))
    }

    async fn direct_space(&self) -> RepoResult<Option<ServerId>> {
        Ok(self
            .one::<UuidRow>("SELECT id FROM server WHERE kind = 'direct'", vec![])
            .await?
            .map(|r| r.id.into()))
    }

    async fn join_as_guest(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
        Ok(self
            .execute(
                "INSERT INTO server_member (id, server_id, user_id, member_role, joined_at) \
                 SELECT $1, $2, $3, 'guest'::server_member_role, now() \
                 WHERE NOT EXISTS (SELECT 1 FROM server_member \
                                   WHERE server_id = $2 AND user_id = $3) \
                 ON CONFLICT DO NOTHING",
                vec![uuid(Uuid::now_v7()), uuid(server_id), uuid(user_id)],
            )
            .await?
            == 1)
    }

    async fn user_kind(&self, user_id: UserId) -> RepoResult<Option<UserKind>> {
        self.one::<UserKindRow>(
            r#"SELECT id, kind FROM "user" WHERE id = $1"#,
            vec![uuid(user_id)],
        )
        .await?
        .map(|r| UserKind::parse(&r.kind).ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData)))
        .transpose()
    }

    async fn find_user(&self, principal_id: &str) -> RepoResult<Option<(UserId, UserKind)>> {
        self.one::<UserKindRow>(
            r#"SELECT id, kind FROM "user" WHERE principal_id = $1 AND deleted_at IS NULL"#,
            vec![principal_id.into()],
        )
        .await?
        .map(|r| {
            UserKind::parse(&r.kind)
                .map(|kind| (r.id.into(), kind))
                .ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData))
        })
        .transpose()
    }

    async fn create_remote_user(&self, user: NewRemoteUser) -> RepoResult<UserId> {
        let id = UserId::new();
        self.execute(
            r#"INSERT INTO "user" (id, username, password_hash, display_name, email, kind,
                   principal_id, identity_key, home_relay, created_at, updated_at)
               VALUES ($1, $2, NULL, $3, NULL, 'remote', $4, $5, $6, now(), now())"#,
            vec![
                uuid(id),
                user.username.into(),
                user.display_name.into(),
                user.principal_id.into(),
                user.identity_key.into(),
                user.home_relay.into(),
            ],
        )
        .await?;
        Ok(id)
    }

    async fn update_remote_user(
        &self,
        user_id: UserId,
        display_name: Option<String>,
        home_relay: Option<String>,
    ) -> RepoResult<()> {
        self.execute(
            r#"UPDATE "user" SET display_name = COALESCE($2, display_name),
                   home_relay = COALESCE($3, home_relay), updated_at = now()
               WHERE id = $1 AND kind = 'remote'"#,
            vec![uuid(user_id), display_name.into(), home_relay.into()],
        )
        .await?;
        Ok(())
    }

    async fn make_local(
        &self,
        user_id: UserId,
        username: &str,
        display_name: &str,
        email: Option<&str>,
    ) -> RepoResult<()> {
        let changed = self
            .execute(
                r#"UPDATE "user" SET kind = 'local', username = $2, display_name = $3,
                       email = COALESCE($4, email), home_relay = NULL, home_username = NULL,
                       home_handle_domain = NULL, updated_at = now()
                   WHERE id = $1 AND kind = 'remote'"#,
                vec![
                    uuid(user_id),
                    username.into(),
                    display_name.into(),
                    email.map(str::to_string).into(),
                ],
            )
            .await?;
        if changed == 1 {
            Ok(())
        } else {
            Err(RepoError::new(RepoErrorKind::Conflict))
        }
    }

    async fn home_relay(&self, user_id: UserId) -> RepoResult<Option<String>> {
        Ok(self
            .one::<OptTextRow>(
                r#"SELECT home_relay AS value FROM "user" WHERE id = $1"#,
                vec![uuid(user_id)],
            )
            .await?
            .and_then(|r| r.value))
    }

    async fn set_home_handle(&self, user_id: UserId, handle: Option<HomeHandle>) -> RepoResult<()> {
        let (username, domain) = match handle {
            Some(h) => (Some(h.username), Some(h.domain)),
            None => (None, None),
        };
        self.execute(
            r#"UPDATE "user" SET home_username = $2, home_handle_domain = $3, updated_at = now()
               WHERE id = $1 AND kind = 'remote'
                 AND (home_username, home_handle_domain) IS DISTINCT FROM ($2, $3)"#,
            vec![uuid(user_id), username.into(), domain.into()],
        )
        .await?;
        Ok(())
    }

    async fn peer_at_origin(&self, origin: &str) -> RepoResult<Option<String>> {
        Ok(self
            .one::<TextRow>(
                "SELECT server_id AS value FROM peer_server \
                 WHERE origin = $1 AND status = 'active' ORDER BY fetched_at DESC LIMIT 1",
                vec![origin.into()],
            )
            .await?
            .map(|r| r.value))
    }

    async fn remote_members(&self, server_id: ServerId) -> RepoResult<Vec<RemoteMember>> {
        self.all::<RemoteMemberRow>(
            r#"SELECT u.id AS user_id, u.principal_id, sm.member_role::text AS role, u.home_relay,
                      pc.payload AS card_payload
               FROM server_member sm
               JOIN "user" u ON u.id = sm.user_id
               LEFT JOIN profile_card pc ON pc.user_id = u.id
               WHERE sm.server_id = $1 AND u.kind = 'remote' AND u.principal_id IS NOT NULL
               ORDER BY u.id"#,
            vec![uuid(server_id)],
        )
        .await?
        .into_iter()
        .map(|r| {
            Ok(RemoteMember {
                user_id: r.user_id.into(),
                principal_id: r.principal_id,
                role: parse_role(&r.role)?,
                home_relay: r.home_relay,
                card_payload: r.card_payload,
            })
        })
        .collect()
    }

    async fn set_device_mls_active(
        &self,
        device_key_id: DeviceKeyId,
        active: bool,
    ) -> RepoResult<()> {
        self.execute(
            "UPDATE device_key SET last_key_package_upload_at = \
                 CASE WHEN $2 THEN now() ELSE NULL END \
             WHERE id = $1",
            vec![uuid(device_key_id), active.into()],
        )
        .await?;
        Ok(())
    }

    async fn shares_workspace(
        &self,
        user_id: UserId,
        principal_id: &str,
        except: Option<ServerId>,
    ) -> RepoResult<bool> {
        Ok(self
            .one::<FlagRow>(
                r#"SELECT EXISTS (
                       SELECT 1 FROM server_member a
                       JOIN server s ON s.id = a.server_id AND s.kind = 'workspace'
                       JOIN server_member b ON b.server_id = a.server_id
                       JOIN "user" u ON u.id = b.user_id
                       WHERE a.user_id = $1 AND u.principal_id = $2
                         AND a.server_id IS DISTINCT FROM $3::uuid) AS flag"#,
                vec![
                    uuid(user_id),
                    principal_id.into(),
                    except.map(Uuid::from).into(),
                ],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn shares_workspace_with_account(
        &self,
        user_id: UserId,
        other: UserId,
        except: Option<ServerId>,
    ) -> RepoResult<bool> {
        Ok(self
            .one::<FlagRow>(
                r#"SELECT EXISTS (
                       SELECT 1 FROM server_member a
                       JOIN server s ON s.id = a.server_id AND s.kind = 'workspace'
                       JOIN server_member b ON b.server_id = a.server_id
                       WHERE a.user_id = $1 AND b.user_id = $2
                         AND a.server_id IS DISTINCT FROM $3::uuid) AS flag"#,
                vec![uuid(user_id), uuid(other), except.map(Uuid::from).into()],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn set_conversation_predecessor(
        &self,
        conversation_id: ConversationId,
        host_server_id: &str,
        predecessor_id: Uuid,
    ) -> RepoResult<()> {
        self.execute(
            "UPDATE conversation SET predecessor_host = $2, predecessor_conversation_id = $3 \
             WHERE id = $1",
            vec![
                uuid(conversation_id),
                host_server_id.into(),
                predecessor_id.into(),
            ],
        )
        .await?;
        Ok(())
    }

    async fn conversation_predecessor(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<(String, Uuid)>> {
        Ok(self
            .one::<PredecessorRow>(
                "SELECT predecessor_host, predecessor_conversation_id FROM conversation \
                 WHERE id = $1",
                vec![uuid(conversation_id)],
            )
            .await?
            .and_then(|r| r.predecessor_host.zip(r.predecessor_conversation_id)))
    }

    async fn next_stream_seq(&self, workspace_id: ServerId, stream: &str) -> RepoResult<i64> {
        self.one::<I64Row>(
            "INSERT INTO host_event_stream (workspace_id, stream, seq) VALUES ($1, $2, 1) \
             ON CONFLICT (workspace_id, stream) DO UPDATE SET seq = host_event_stream.seq + 1 \
             RETURNING seq AS n",
            vec![uuid(workspace_id), stream.into()],
        )
        .await?
        .map(|r| r.n)
        .ok_or_else(|| internal("no stream seq"))
    }

    async fn insert_outbox(
        &self,
        peer_server_id: &str,
        item: &str,
        size_bytes: i32,
    ) -> RepoResult<i64> {
        self.one::<I64Row>(
            "INSERT INTO s2s_outbox (peer_server_id, item, size_bytes) VALUES ($1, $2, $3) \
             RETURNING id AS n",
            vec![peer_server_id.into(), item.into(), size_bytes.into()],
        )
        .await?
        .map(|r| r.n)
        .ok_or_else(|| internal("no outbox id"))
    }

    async fn outbox_peers_due(&self, now: DateTime<Utc>) -> RepoResult<Vec<String>> {
        Ok(self
            .all::<TextRow>(
                "SELECT DISTINCT o.peer_server_id AS value FROM s2s_outbox o \
                 LEFT JOIN s2s_peer_delivery d ON d.peer_server_id = o.peer_server_id \
                 WHERE o.id > COALESCE(d.delivered_through, 0) \
                   AND COALESCE(d.next_attempt_at, $1) <= $1 \
                 UNION \
                 SELECT peer_server_id FROM s2s_peer_delivery \
                 WHERE failures > 0 AND next_attempt_at <= $1",
                vec![now.into()],
            )
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    }

    async fn outbox_after(
        &self,
        peer_server_id: &str,
        after: i64,
        limit: i64,
        max_bytes: i64,
    ) -> RepoResult<Vec<OutboxRow>> {
        Ok(self
            .all::<OutboxDbRow>(
                "WITH page AS (
                     SELECT id, item, size_bytes FROM s2s_outbox
                     WHERE peer_server_id = $1 AND id > $2 ORDER BY id LIMIT $3),
                 ranked AS (
                     SELECT id, item, size_bytes,
                            sum(size_bytes) OVER (ORDER BY id) AS running,
                            row_number() OVER (ORDER BY id) AS rn
                     FROM page)
                 SELECT id, item, size_bytes FROM ranked
                 WHERE rn = 1 OR running <= $4 ORDER BY id",
                vec![
                    peer_server_id.into(),
                    after.into(),
                    limit.into(),
                    max_bytes.into(),
                ],
            )
            .await?
            .into_iter()
            .map(|r| OutboxRow {
                id: r.id,
                item: r.item,
                size_bytes: r.size_bytes,
            })
            .collect())
    }

    async fn delivery(&self, peer_server_id: &str) -> RepoResult<PeerDelivery> {
        Ok(self
            .one::<DeliveryRow>(
                "SELECT delivered_through, failures, next_attempt_at, failing_since \
                 FROM s2s_peer_delivery WHERE peer_server_id = $1",
                vec![peer_server_id.into()],
            )
            .await?
            .map(|r| PeerDelivery {
                delivered_through: r.delivered_through,
                failures: r.failures,
                next_attempt_at: r.next_attempt_at.into(),
                failing_since: r.failing_since.map(Into::into),
            })
            .unwrap_or(PeerDelivery {
                delivered_through: 0,
                failures: 0,
                next_attempt_at: Utc::now(),
                failing_since: None,
            }))
    }

    async fn outbox_backlog(&self, peer_server_id: &str) -> RepoResult<OutboxBacklog> {
        self.one::<BacklogRow>(
            "WITH d AS ( \
                 SELECT COALESCE(max(delivered_through), 0) AS through, \
                        max(failing_since) AS failing_since \
                 FROM s2s_peer_delivery WHERE peer_server_id = $1) \
             SELECT count(o.id) AS rows, COALESCE(sum(o.size_bytes), 0)::bigint AS bytes, \
                    d.failing_since \
             FROM d LEFT JOIN s2s_outbox o ON o.peer_server_id = $1 AND o.id > d.through \
             GROUP BY d.failing_since",
            vec![peer_server_id.into()],
        )
        .await?
        .map(|r| OutboxBacklog {
            rows: r.rows,
            bytes: r.bytes,
            failing_since: r.failing_since.map(Into::into),
        })
        .ok_or_else(|| internal("no outbox backlog"))
    }

    async fn outbox_stats(&self, dead_before: DateTime<Utc>) -> RepoResult<OutboxStats> {
        self.one::<OutboxStatsRow>(
            "WITH backlog AS ( \
                 SELECT o.peer_server_id, count(*) AS rows, sum(o.size_bytes)::bigint AS bytes \
                 FROM s2s_outbox o \
                 LEFT JOIN s2s_peer_delivery d ON d.peer_server_id = o.peer_server_id \
                 WHERE o.id > COALESCE(d.delivered_through, 0) \
                 GROUP BY o.peer_server_id) \
             SELECT COALESCE(sum(rows), 0)::bigint AS rows, \
                    COALESCE(sum(bytes), 0)::bigint AS bytes, \
                    COALESCE(max(rows), 0)::bigint AS peer_max_rows, \
                    COALESCE(max(bytes), 0)::bigint AS peer_max_bytes, \
                    (SELECT count(*) FROM s2s_peer_delivery \
                     WHERE failing_since <= $1) AS dead_peers \
             FROM backlog",
            vec![dead_before.into()],
        )
        .await?
        .map(|r| OutboxStats {
            rows: r.rows,
            bytes: r.bytes,
            peer_max_rows: r.peer_max_rows,
            peer_max_bytes: r.peer_max_bytes,
            dead_peers: r.dead_peers,
        })
        .ok_or_else(|| internal("no outbox stats"))
    }

    async fn record_delivered(&self, peer_server_id: &str, through: i64) -> RepoResult<()> {
        self.execute(
            "INSERT INTO s2s_peer_delivery (peer_server_id, delivered_through, failures, \
                 next_attempt_at, last_success_at) VALUES ($1, $2, 0, now(), now()) \
             ON CONFLICT (peer_server_id) DO UPDATE SET \
                 delivered_through = GREATEST(s2s_peer_delivery.delivered_through, \
                                              EXCLUDED.delivered_through), \
                 failures = 0, next_attempt_at = now(), last_success_at = now(), \
                 failing_since = NULL",
            vec![peer_server_id.into(), through.into()],
        )
        .await?;
        Ok(())
    }

    async fn record_failure(
        &self,
        peer_server_id: &str,
        failures: i32,
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.execute(
            "INSERT INTO s2s_peer_delivery (peer_server_id, failures, next_attempt_at, \
                 failing_since) VALUES ($1, $2, $3, now()) \
             ON CONFLICT (peer_server_id) DO UPDATE SET \
                 failures = EXCLUDED.failures, next_attempt_at = EXCLUDED.next_attempt_at, \
                 failing_since = COALESCE(s2s_peer_delivery.failing_since, now())",
            vec![
                peer_server_id.into(),
                failures.into(),
                next_attempt_at.into(),
            ],
        )
        .await?;
        Ok(())
    }

    async fn stream_heads(
        &self,
        workspace_id: ServerId,
        streams: &[String],
    ) -> RepoResult<Vec<(String, i64)>> {
        Ok(self
            .all::<HeadRow>(
                "SELECT stream, seq FROM host_event_stream \
                 WHERE workspace_id = $1 AND stream = ANY($2)",
                vec![uuid(workspace_id), streams.to_vec().into()],
            )
            .await?
            .into_iter()
            .map(|r| (r.stream, r.seq))
            .collect())
    }

    async fn delete_outbox_before(&self, before: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM s2s_outbox WHERE created_at < $1",
            vec![before.into()],
        )
        .await
    }

    async fn check_request_seq(
        &self,
        device_key_id: DeviceKeyId,
        seq: i64,
        window: i64,
        seen_until: DateTime<Utc>,
    ) -> RepoResult<SeqDecision> {
        let device: Uuid = device_key_id.into();
        self.db
            .transaction::<_, SeqDecision, DbErr>(|txn| {
                Box::pin(async move {
                    let highest = HighestRow::find_by_statement(statement(
                        "SELECT request_highest_seq FROM device_key WHERE id = $1 FOR UPDATE",
                        vec![device.into()],
                    ))
                    .one(txn)
                    .await?
                    .ok_or_else(|| DbErr::RecordNotFound("device_key".to_string()))?
                    .request_highest_seq;
                    if let Some(highest) = highest
                        && seq <= highest.saturating_sub(window)
                    {
                        return Ok(SeqDecision::Replay { highest });
                    }
                    let inserted = txn
                        .execute_raw(statement(
                            "INSERT INTO device_request_seen (device_key_id, seq, expires_at) \
                             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                            vec![device.into(), seq.into(), seen_until.into()],
                        ))
                        .await?
                        .rows_affected();
                    if inserted == 0 {
                        return Ok(SeqDecision::Replay {
                            highest: highest.unwrap_or(seq).max(seq),
                        });
                    }
                    txn.execute_raw(statement(
                        "UPDATE device_key SET request_highest_seq = \
                             GREATEST(COALESCE(request_highest_seq, $2), $2) WHERE id = $1",
                        vec![device.into(), seq.into()],
                    ))
                    .await?;
                    Ok(SeqDecision::Accepted)
                })
            })
            .await
            .map_err(|e| match e {
                TransactionError::Connection(e) | TransactionError::Transaction(e) => map_db_err(e),
            })
    }

    async fn purge_request_seen(&self, now: DateTime<Utc>, window: i64) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM device_request_seen s USING device_key d \
             WHERE d.id = s.device_key_id AND s.expires_at < $1 \
               AND s.seq <= COALESCE(d.request_highest_seq, 0) - $2",
            vec![now.into(), window.into()],
        )
        .await
    }
}
