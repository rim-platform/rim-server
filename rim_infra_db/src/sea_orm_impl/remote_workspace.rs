//! Workspace members whose relay is another server: the host's delegations per workspace,
//! device and relay, and the relay's subscriptions and revocation fan-outs, in raw SQL.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::host::model::{DelegationStored, NewDelegation, RemoteMember};
use rim_core::host::repository::DelegationRepository;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::relay::model::{
    DueFanout, FanoutStatus, RelaySubscription, RevocationFanout, SubscriptionStatus,
};
use rim_core::relay::repository::SubscriptionRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn uuid<T: Into<Uuid>>(id: T) -> Value {
    let id: Uuid = id.into();
    id.into()
}

fn time(at: DateTime<Utc>) -> Value {
    DateTimeWithTimeZone::from(at).into()
}

fn invalid() -> RepoError {
    RepoError::new(RepoErrorKind::InvalidData)
}

fn parse_role(role: &str) -> RepoResult<ServerMemberRole> {
    match role {
        "owner" => Ok(ServerMemberRole::Owner),
        "moderator" => Ok(ServerMemberRole::Moderator),
        "member" => Ok(ServerMemberRole::Member),
        "guest" => Ok(ServerMemberRole::Guest),
        _ => Err(invalid()),
    }
}

/// Shared query helpers over a pool connection or a transaction.
struct Sql<'a>(&'a DbHandle);

impl Sql<'_> {
    async fn all<T: FromQueryResult>(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<T>> {
        T::find_by_statement(statement(sql, values))
            .all(self.0)
            .await
            .map_err(map_db_err)
    }

    async fn one<T: FromQueryResult>(
        &self,
        sql: &str,
        values: Vec<Value>,
    ) -> RepoResult<Option<T>> {
        T::find_by_statement(statement(sql, values))
            .one(self.0)
            .await
            .map_err(map_db_err)
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        self.0
            .execute_raw(statement(sql, values))
            .await
            .map(|res| res.rows_affected())
            .map_err(map_db_err)
    }
}

#[derive(FromQueryResult)]
struct ExpiresRow {
    expires_at: DateTimeWithTimeZone,
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

#[derive(FromQueryResult)]
struct FlagRow {
    flag: bool,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct TextRow {
    value: String,
}

#[derive(FromQueryResult)]
struct UserRelayRow {
    user_id: Uuid,
    relay_server_id: String,
}

#[derive(FromQueryResult)]
struct MemberRow {
    user_id: Uuid,
    principal_id: String,
    role: String,
    home_relay: Option<String>,
    card_payload: Option<String>,
}

/// Delegations relays hold for this host's workspaces.
pub struct SeaOrmDelegationRepository {
    db: DbHandle,
}

impl SeaOrmDelegationRepository {
    /// A repository on `db`, a pool connection or a transaction.
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn sql(&self) -> Sql<'_> {
        Sql(&self.db)
    }
}

/// An active delegation: not ended and not expired at `$now`.
const ACTIVE: &str = "d.ended_at IS NULL AND d.expires_at > $NOW";

fn active(now_param: usize) -> String {
    ACTIVE.replace("$NOW", &format!("${now_param}"))
}

#[async_trait]
impl DelegationRepository for SeaOrmDelegationRepository {
    async fn store(
        &self,
        delegation: NewDelegation,
        now: DateTime<Utc>,
    ) -> RepoResult<DelegationStored> {
        let stored = self
            .sql()
            .one::<ExpiresRow>(
                "INSERT INTO host_delegation (id, workspace_id, user_id, device_key_id, \
                     relay_server_id, payload, signature, accepted_at, expires_at, ended_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL) \
                 ON CONFLICT (workspace_id, device_key_id, relay_server_id) DO UPDATE SET \
                     user_id = EXCLUDED.user_id, payload = EXCLUDED.payload, \
                     signature = EXCLUDED.signature, accepted_at = EXCLUDED.accepted_at, \
                     expires_at = EXCLUDED.expires_at, ended_at = NULL \
                 WHERE host_delegation.ended_at IS NOT NULL \
                    OR host_delegation.expires_at <= EXCLUDED.accepted_at \
                    OR host_delegation.expires_at <= EXCLUDED.expires_at \
                 RETURNING expires_at",
                vec![
                    uuid(Uuid::now_v7()),
                    uuid(delegation.workspace_id),
                    uuid(delegation.user_id),
                    uuid(delegation.device_key_id),
                    delegation.relay_server_id.clone().into(),
                    delegation.payload.into(),
                    delegation.signature.into(),
                    time(now),
                    time(delegation.expires_at),
                ],
            )
            .await?;
        if let Some(row) = stored {
            return Ok(DelegationStored::Stored {
                expires_at: row.expires_at.with_timezone(&Utc),
            });
        }
        let kept = self
            .sql()
            .one::<ExpiresRow>(
                "SELECT expires_at FROM host_delegation \
                 WHERE workspace_id = $1 AND device_key_id = $2 AND relay_server_id = $3",
                vec![
                    uuid(delegation.workspace_id),
                    uuid(delegation.device_key_id),
                    delegation.relay_server_id.into(),
                ],
            )
            .await?
            .ok_or_else(invalid)?;
        Ok(DelegationStored::Kept {
            expires_at: kept.expires_at.with_timezone(&Utc),
        })
    }

    async fn is_active(
        &self,
        workspace_id: ServerId,
        device_key_id: DeviceKeyId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self
            .sql()
            .one::<FlagRow>(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM host_delegation d \
                     WHERE d.workspace_id = $1 AND d.device_key_id = $2 \
                       AND d.relay_server_id = $3 AND {}) AS flag",
                    active(4)
                ),
                vec![
                    uuid(workspace_id),
                    uuid(device_key_id),
                    relay_server_id.into(),
                    time(now),
                ],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn count_active_for_relay(
        &self,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<i64> {
        Ok(self
            .sql()
            .one::<CountRow>(
                &format!(
                    "SELECT count(*) AS n FROM host_delegation d \
                     WHERE d.relay_server_id = $1 AND {}",
                    active(2)
                ),
                vec![relay_server_id.into(), time(now)],
            )
            .await?
            .map_or(0, |r| r.n))
    }

    async fn device_delegated(
        &self,
        device_key_id: DeviceKeyId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self
            .sql()
            .one::<FlagRow>(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM host_delegation d \
                       JOIN device_key k ON k.id = d.device_key_id \
                       JOIN server_member sm ON sm.server_id = d.workspace_id \
                                            AND sm.user_id = d.user_id \
                     WHERE d.device_key_id = $1 AND d.relay_server_id = $2 \
                       AND k.revoked_at IS NULL AND {}) AS flag",
                    active(3)
                ),
                vec![uuid(device_key_id), relay_server_id.into(), time(now)],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn delegated_members(
        &self,
        workspace_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<RemoteMember>> {
        self.sql()
            .all::<MemberRow>(
                &format!(
                    r#"SELECT DISTINCT u.id AS user_id, u.principal_id,
                              sm.member_role::text AS role, u.home_relay,
                              pc.payload AS card_payload
                       FROM host_delegation d
                       JOIN server_member sm ON sm.server_id = d.workspace_id
                                            AND sm.user_id = d.user_id
                       JOIN "user" u ON u.id = d.user_id
                       LEFT JOIN profile_card pc ON pc.user_id = u.id
                       WHERE d.workspace_id = $1 AND u.principal_id IS NOT NULL AND {}
                       ORDER BY user_id"#,
                    active(2)
                ),
                vec![uuid(workspace_id), time(now)],
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

    async fn delegated_relays(
        &self,
        workspace_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<(UserId, String)>> {
        Ok(self
            .sql()
            .all::<UserRelayRow>(
                &format!(
                    "SELECT DISTINCT d.user_id, d.relay_server_id FROM host_delegation d \
                     WHERE d.workspace_id = $1 AND {} ORDER BY d.user_id, d.relay_server_id",
                    active(2)
                ),
                vec![uuid(workspace_id), time(now)],
            )
            .await?
            .into_iter()
            .map(|r| (r.user_id.into(), r.relay_server_id))
            .collect())
    }

    async fn delegated_workspaces(
        &self,
        user_id: UserId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<ServerId>> {
        Ok(self
            .sql()
            .all::<IdRow>(
                &format!(
                    "SELECT DISTINCT d.workspace_id AS id FROM host_delegation d \
                       JOIN server_member sm ON sm.server_id = d.workspace_id \
                                            AND sm.user_id = d.user_id \
                     WHERE d.user_id = $1 AND d.relay_server_id = $2 AND {} ORDER BY id",
                    active(3)
                ),
                vec![uuid(user_id), relay_server_id.into(), time(now)],
            )
            .await?
            .into_iter()
            .map(|r| r.id.into())
            .collect())
    }

    async fn end_for_user<'a>(
        &self,
        workspace_id: ServerId,
        user_id: UserId,
        relay_server_id: Option<&'a str>,
    ) -> RepoResult<u64> {
        match relay_server_id {
            Some(relay) => {
                self.sql()
                    .execute(
                        "UPDATE host_delegation SET ended_at = now() \
                         WHERE workspace_id = $1 AND user_id = $2 AND relay_server_id = $3 \
                           AND ended_at IS NULL",
                        vec![uuid(workspace_id), uuid(user_id), relay.into()],
                    )
                    .await
            }
            None => {
                self.sql()
                    .execute(
                        "UPDATE host_delegation SET ended_at = now() \
                         WHERE workspace_id = $1 AND user_id = $2 AND ended_at IS NULL",
                        vec![uuid(workspace_id), uuid(user_id)],
                    )
                    .await
            }
        }
    }

    async fn end_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64> {
        self.sql()
            .execute(
                "UPDATE host_delegation SET ended_at = now() \
                 WHERE device_key_id = $1 AND ended_at IS NULL",
                vec![uuid(device_key_id)],
            )
            .await
    }

    async fn end_lapsed(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        self.sql()
            .execute(
                "UPDATE host_delegation d SET ended_at = now() \
                 WHERE d.ended_at IS NULL AND (d.expires_at <= $1 \
                    OR EXISTS (SELECT 1 FROM device_key k \
                               WHERE k.id = d.device_key_id AND k.revoked_at IS NOT NULL) \
                    OR NOT EXISTS (SELECT 1 FROM server_member sm \
                                   WHERE sm.server_id = d.workspace_id AND sm.user_id = d.user_id))",
                vec![time(now)],
            )
            .await
    }
}

#[derive(FromQueryResult)]
struct SubscriptionRow {
    user_id: Uuid,
    host_server_id: String,
    workspace_id: Uuid,
    status: String,
    expires_at: Option<DateTimeWithTimeZone>,
    updated_at: DateTimeWithTimeZone,
}

impl TryFrom<SubscriptionRow> for RelaySubscription {
    type Error = RepoError;

    fn try_from(r: SubscriptionRow) -> RepoResult<Self> {
        Ok(Self {
            user_id: r.user_id.into(),
            host_server_id: r.host_server_id,
            workspace_id: r.workspace_id,
            status: SubscriptionStatus::parse(&r.status).ok_or_else(invalid)?,
            expires_at: r.expires_at.map(|t| t.with_timezone(&Utc)),
            updated_at: r.updated_at.with_timezone(&Utc),
        })
    }
}

#[derive(FromQueryResult)]
struct FanoutRow {
    device_key_id: Uuid,
    host_server_id: String,
    status: String,
    attempts: i32,
    next_attempt_at: DateTimeWithTimeZone,
    ack_payload: Option<String>,
    ack_signature: Option<String>,
}

impl TryFrom<FanoutRow> for RevocationFanout {
    type Error = RepoError;

    fn try_from(r: FanoutRow) -> RepoResult<Self> {
        Ok(Self {
            device_key_id: r.device_key_id.into(),
            host_server_id: r.host_server_id,
            status: FanoutStatus::parse(&r.status).ok_or_else(invalid)?,
            attempts: r.attempts,
            next_attempt_at: r.next_attempt_at.with_timezone(&Utc),
            ack_payload: r.ack_payload,
            ack_signature: r.ack_signature,
        })
    }
}

#[derive(FromQueryResult)]
struct DueFanoutRow {
    device_key_id: Uuid,
    host_server_id: String,
    status: String,
    attempts: i32,
    next_attempt_at: DateTimeWithTimeZone,
    ack_payload: Option<String>,
    ack_signature: Option<String>,
    user_id: Uuid,
    principal_id: String,
    revocation_payload: String,
    revocation_signature: String,
    revoked_at: DateTimeWithTimeZone,
}

const SUBSCRIPTION_COLUMNS: &str =
    "user_id, host_server_id, workspace_id, status, expires_at, updated_at";

const FANOUT_COLUMNS: &str = "f.device_key_id, f.host_server_id, f.status, f.attempts, \
     f.next_attempt_at, f.ack_payload, f.ack_signature";

/// A relay's subscriptions to workspaces elsewhere and its revocation fan-outs.
pub struct SeaOrmSubscriptionRepository {
    db: DbHandle,
}

impl SeaOrmSubscriptionRepository {
    /// A repository on `db`, a pool connection or a transaction.
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn sql(&self) -> Sql<'_> {
        Sql(&self.db)
    }
}

#[async_trait]
impl SubscriptionRepository for SeaOrmSubscriptionRepository {
    async fn upsert_subscription(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        status: SubscriptionStatus,
        expires_at: Option<DateTime<Utc>>,
    ) -> RepoResult<RelaySubscription> {
        // Another device's active delegation keeps an active subscription active, and the
        // later of two expiries wins.
        self.sql()
            .one::<SubscriptionRow>(
                &format!(
                    "INSERT INTO relay_subscription (id, user_id, host_server_id, workspace_id, \
                         status, expires_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, now()) \
                     ON CONFLICT (user_id, host_server_id, workspace_id) DO UPDATE SET \
                       status = CASE WHEN relay_subscription.status = 'active' \
                                      AND relay_subscription.expires_at > now() \
                                      AND EXCLUDED.status <> 'active' \
                                     THEN relay_subscription.status ELSE EXCLUDED.status END, \
                       expires_at = CASE WHEN relay_subscription.status = 'active' \
                                          AND relay_subscription.expires_at > now() \
                                         THEN GREATEST(relay_subscription.expires_at, \
                                                       EXCLUDED.expires_at) \
                                         ELSE EXCLUDED.expires_at END, \
                       updated_at = now() \
                     RETURNING {SUBSCRIPTION_COLUMNS}"
                ),
                vec![
                    uuid(Uuid::now_v7()),
                    uuid(user_id),
                    host_server_id.into(),
                    uuid(workspace_id),
                    status.as_str().into(),
                    expires_at.map(DateTimeWithTimeZone::from).into(),
                ],
            )
            .await?
            .ok_or_else(invalid)?
            .try_into()
    }

    async fn list_subscriptions(&self, user_id: UserId) -> RepoResult<Vec<RelaySubscription>> {
        self.sql()
            .all::<SubscriptionRow>(
                &format!(
                    "SELECT {SUBSCRIPTION_COLUMNS} FROM relay_subscription \
                     WHERE user_id = $1 ORDER BY id"
                ),
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    async fn is_subscribed(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self
            .sql()
            .one::<FlagRow>(
                "SELECT EXISTS (SELECT 1 FROM relay_subscription \
                   WHERE user_id = $1 AND host_server_id = $2 AND workspace_id = $3 \
                     AND status = 'active' AND (expires_at IS NULL OR expires_at > $4)) AS flag",
                vec![
                    uuid(user_id),
                    host_server_id.into(),
                    uuid(workspace_id),
                    time(now),
                ],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn subscribed_hosts(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<String>> {
        Ok(self
            .sql()
            .all::<TextRow>(
                "SELECT DISTINCT host_server_id AS value FROM relay_subscription \
                 WHERE user_id = $1 AND status = 'active' \
                   AND (expires_at IS NULL OR expires_at > $2) ORDER BY value",
                vec![uuid(user_id), time(now)],
            )
            .await?
            .into_iter()
            .map(|r| r.value)
            .collect())
    }

    async fn end_subscriptions(&self, user_id: UserId) -> RepoResult<u64> {
        self.sql()
            .execute(
                "UPDATE relay_subscription SET status = 'ended', updated_at = now() \
                 WHERE user_id = $1 AND status <> 'ended'",
                vec![uuid(user_id)],
            )
            .await
    }

    async fn lapse_expired(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        self.sql()
            .execute(
                "UPDATE relay_subscription SET status = 'lapsed', updated_at = now() \
                 WHERE status = 'active' AND expires_at <= $1",
                vec![time(now)],
            )
            .await
    }

    async fn record_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        request_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.sql()
            .execute(
                "INSERT INTO relay_join_request (user_id, host_server_id, workspace_id, \
                     request_id, created_at, expires_at) \
                 VALUES ($1, $2, $3, $4, now(), $5) \
                 ON CONFLICT (user_id, host_server_id, workspace_id) DO UPDATE SET \
                   request_id = EXCLUDED.request_id, created_at = now(), \
                   expires_at = EXCLUDED.expires_at",
                vec![
                    uuid(user_id),
                    host_server_id.into(),
                    uuid(workspace_id),
                    uuid(request_id),
                    time(expires_at),
                ],
            )
            .await?;
        Ok(())
    }

    async fn has_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self
            .sql()
            .one::<FlagRow>(
                "SELECT EXISTS (SELECT 1 FROM relay_join_request \
                   WHERE user_id = $1 AND host_server_id = $2 AND workspace_id = $3 \
                     AND expires_at > $4) AS flag",
                vec![
                    uuid(user_id),
                    host_server_id.into(),
                    uuid(workspace_id),
                    time(now),
                ],
            )
            .await?
            .is_some_and(|r| r.flag))
    }

    async fn delete_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
    ) -> RepoResult<bool> {
        Ok(self
            .sql()
            .execute(
                "DELETE FROM relay_join_request \
                 WHERE user_id = $1 AND host_server_id = $2 AND workspace_id = $3",
                vec![uuid(user_id), host_server_id.into(), uuid(workspace_id)],
            )
            .await?
            > 0)
    }

    async fn delete_join_requests(&self, user_id: UserId) -> RepoResult<u64> {
        self.sql()
            .execute(
                "DELETE FROM relay_join_request WHERE user_id = $1",
                vec![uuid(user_id)],
            )
            .await
    }

    async fn delete_expired_join_requests(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        self.sql()
            .execute(
                "DELETE FROM relay_join_request WHERE expires_at <= $1",
                vec![time(now)],
            )
            .await
    }

    async fn insert_fanouts(
        &self,
        device_key_id: DeviceKeyId,
        hosts: &[String],
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        let mut added = 0;
        for host in hosts {
            added += self
                .sql()
                .execute(
                    "INSERT INTO relay_revocation_fanout (device_key_id, host_server_id, status, \
                         attempts, next_attempt_at) \
                     VALUES ($1, $2, 'pending', 0, $3) ON CONFLICT DO NOTHING",
                    vec![
                        uuid(device_key_id),
                        host.as_str().into(),
                        time(next_attempt_at),
                    ],
                )
                .await?;
        }
        Ok(added)
    }

    async fn due_fanouts(&self, now: DateTime<Utc>, limit: i64) -> RepoResult<Vec<DueFanout>> {
        self.sql()
            .all::<DueFanoutRow>(
                &format!(
                    r#"SELECT {FANOUT_COLUMNS}, k.user_id, u.principal_id,
                              k.revocation_payload, k.revocation_signature, k.revoked_at
                       FROM relay_revocation_fanout f
                       JOIN device_key k ON k.id = f.device_key_id
                       JOIN "user" u ON u.id = k.user_id
                       WHERE f.status = 'pending' AND f.next_attempt_at <= $1
                         AND k.revocation_payload IS NOT NULL
                         AND k.revocation_signature IS NOT NULL
                         AND k.revoked_at IS NOT NULL AND u.principal_id IS NOT NULL
                       ORDER BY f.next_attempt_at LIMIT $2"#
                ),
                vec![time(now), limit.into()],
            )
            .await?
            .into_iter()
            .map(|r| {
                Ok(DueFanout {
                    fanout: FanoutRow {
                        device_key_id: r.device_key_id,
                        host_server_id: r.host_server_id,
                        status: r.status,
                        attempts: r.attempts,
                        next_attempt_at: r.next_attempt_at,
                        ack_payload: r.ack_payload,
                        ack_signature: r.ack_signature,
                    }
                    .try_into()?,
                    user_id: r.user_id.into(),
                    principal_id: r.principal_id,
                    revocation_payload: r.revocation_payload,
                    revocation_signature: r.revocation_signature,
                    revoked_at: r.revoked_at.with_timezone(&Utc),
                })
            })
            .collect()
    }

    async fn record_fanout_attempt(
        &self,
        device_key_id: DeviceKeyId,
        host_server_id: &str,
        attempts: i32,
        status: FanoutStatus,
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.sql()
            .execute(
                "UPDATE relay_revocation_fanout SET attempts = $3, status = $4, \
                     next_attempt_at = $5 \
                 WHERE device_key_id = $1 AND host_server_id = $2 AND status = 'pending'",
                vec![
                    uuid(device_key_id),
                    host_server_id.into(),
                    attempts.into(),
                    status.as_str().into(),
                    time(next_attempt_at),
                ],
            )
            .await?;
        Ok(())
    }

    async fn record_fanout_ack(
        &self,
        device_key_id: DeviceKeyId,
        host_server_id: &str,
        attempts: i32,
        ack_payload: &str,
        ack_signature: &str,
    ) -> RepoResult<()> {
        self.sql()
            .execute(
                "UPDATE relay_revocation_fanout SET attempts = $3, status = 'acked', \
                     ack_payload = $4, ack_signature = $5 \
                 WHERE device_key_id = $1 AND host_server_id = $2",
                vec![
                    uuid(device_key_id),
                    host_server_id.into(),
                    attempts.into(),
                    ack_payload.into(),
                    ack_signature.into(),
                ],
            )
            .await?;
        Ok(())
    }

    async fn list_fanouts(&self, device_key_id: DeviceKeyId) -> RepoResult<Vec<RevocationFanout>> {
        self.sql()
            .all::<FanoutRow>(
                &format!(
                    "SELECT {FANOUT_COLUMNS} FROM relay_revocation_fanout f \
                     WHERE f.device_key_id = $1 ORDER BY f.host_server_id"
                ),
                vec![uuid(device_key_id)],
            )
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }
}
