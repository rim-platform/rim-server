use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use entity::message;
use rim_core::conversation::model::ConversationId;
use rim_core::e2ee::model::{
    ClaimedKeyPackage, E2eeDevice, FrankingKey, FrankingRevision, KEY_PACKAGE_MIN_REMAINING_DAYS,
    MlsCommit, MlsCommitId, MlsDesiredDevice, MlsDevice, MlsGroup, MlsGroupDevice, MlsKeyPackage,
    MlsKeyPackageId, MlsKeyPackageStatus, MlsPurged, MlsReaddRequest, MlsReaddRequestId,
    MlsRetention, MlsWelcome, MlsWelcomeId, NewEncryptedMessage, NewFrankingRevision, NewMlsCommit,
    NewMlsGroup, NewMlsKeyPackage, NewMlsReaddRequest, NewMlsWelcome,
};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::message::model::{Message, MessageEncryption, MessageId};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

/// Seconds as `make_interval` takes them.
fn seconds(duration: Duration) -> Value {
    (duration.num_milliseconds() as f64 / 1000.0).into()
}

fn uuids<T: Copy + Into<Uuid>>(ids: &[T]) -> Value {
    ids.iter()
        .copied()
        .map(Into::into)
        .collect::<Vec<Uuid>>()
        .into()
}

fn utc(at: DateTime<Utc>) -> Value {
    at.into()
}

fn internal(message: &str) -> RepoError {
    RepoError::with_message(RepoErrorKind::Internal, message)
}

const DEVICE_COLUMNS: &str = "d.id AS device_key_id, d.user_id, d.device_id, u.principal_id, \
     d.public_key, d.revoked_at, d.last_key_package_upload_at";

#[derive(FromQueryResult)]
struct DeviceRow {
    device_key_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    principal_id: Option<String>,
    public_key: String,
    revoked_at: Option<DateTimeWithTimeZone>,
    last_key_package_upload_at: Option<DateTimeWithTimeZone>,
}

impl From<DeviceRow> for E2eeDevice {
    fn from(row: DeviceRow) -> Self {
        Self {
            device_key_id: row.device_key_id.into(),
            user_id: row.user_id.into(),
            device_id: row.device_id,
            principal_id: row.principal_id,
            public_key: row.public_key,
            revoked_at: row.revoked_at.map(Into::into),
            last_key_package_upload_at: row.last_key_package_upload_at.map(Into::into),
        }
    }
}

#[derive(FromQueryResult)]
struct DesiredDeviceRow {
    device_key_id: Uuid,
    user_id: Uuid,
    principal_id: String,
    device_id: Uuid,
    has_key_package: bool,
}

#[derive(FromQueryResult)]
struct GroupDeviceRow {
    device_key_id: Uuid,
    user_id: Uuid,
    principal_id: String,
    device_id: Uuid,
    added_epoch: i64,
}

#[derive(FromQueryResult)]
struct PrincipalUserRow {
    principal_id: String,
    user_id: Uuid,
}

const KEY_PACKAGE_COLUMNS: &str = "id, user_id, device_key_id, cipher_suite, key_package_ref, \
     last_resort, not_after, created_at, claimed_at, claimed_by_device_key_id, \
     claimed_for_conversation_id, claimed_for_incarnation";

/// A key package that can still be handed out.
fn usable(alias: &str) -> String {
    format!("{alias}not_after > now() + interval '{KEY_PACKAGE_MIN_REMAINING_DAYS} days'")
}

#[derive(FromQueryResult)]
struct KeyPackageRow {
    id: Uuid,
    user_id: Uuid,
    device_key_id: Uuid,
    cipher_suite: i32,
    key_package_ref: Vec<u8>,
    last_resort: bool,
    not_after: DateTimeWithTimeZone,
    created_at: DateTimeWithTimeZone,
    claimed_at: Option<DateTimeWithTimeZone>,
    claimed_by_device_key_id: Option<Uuid>,
    claimed_for_conversation_id: Option<Uuid>,
    claimed_for_incarnation: Option<i32>,
}

impl From<KeyPackageRow> for MlsKeyPackage {
    fn from(row: KeyPackageRow) -> Self {
        Self {
            id: row.id.into(),
            user_id: row.user_id.into(),
            device_key_id: row.device_key_id.into(),
            cipher_suite: row.cipher_suite,
            key_package_ref: row.key_package_ref,
            last_resort: row.last_resort,
            not_after: row.not_after.into(),
            created_at: row.created_at.into(),
            claimed_at: row.claimed_at.map(Into::into),
            claimed_by_device_key_id: row.claimed_by_device_key_id.map(Into::into),
            claimed_for_conversation_id: row.claimed_for_conversation_id.map(Into::into),
            claimed_for_incarnation: row.claimed_for_incarnation,
        }
    }
}

const CLAIMED_COLUMNS: &str = "id, device_key_id, key_package_ref, key_package, last_resort";

#[derive(FromQueryResult)]
struct ClaimedRow {
    id: Uuid,
    device_key_id: Uuid,
    key_package_ref: Vec<u8>,
    key_package: Vec<u8>,
    last_resort: bool,
}

impl From<ClaimedRow> for ClaimedKeyPackage {
    fn from(row: ClaimedRow) -> Self {
        Self {
            id: row.id.into(),
            device_key_id: row.device_key_id.into(),
            key_package_ref: row.key_package_ref,
            key_package: row.key_package,
            last_resort: row.last_resort,
        }
    }
}

#[derive(FromQueryResult)]
struct StatusRow {
    cipher_suite: i32,
    available: i64,
    last_resort_not_after: Option<DateTimeWithTimeZone>,
    last_resort_claims: Option<i32>,
    oldest_not_after: Option<DateTimeWithTimeZone>,
}

const GROUP_COLUMNS: &str = "conversation_id, incarnation, group_id, cipher_suite, epoch, \
     created_by_device_key_id, created_at, retired_at";

#[derive(FromQueryResult)]
struct GroupRow {
    conversation_id: Uuid,
    incarnation: i32,
    group_id: Vec<u8>,
    cipher_suite: i32,
    epoch: i64,
    created_by_device_key_id: Option<Uuid>,
    created_at: DateTimeWithTimeZone,
    retired_at: Option<DateTimeWithTimeZone>,
}

impl From<GroupRow> for MlsGroup {
    fn from(row: GroupRow) -> Self {
        Self {
            conversation_id: row.conversation_id.into(),
            incarnation: row.incarnation,
            group_id: row.group_id,
            cipher_suite: row.cipher_suite,
            epoch: row.epoch,
            created_by_device_key_id: row.created_by_device_key_id.map(Into::into),
            created_at: row.created_at.into(),
            retired_at: row.retired_at.map(Into::into),
        }
    }
}

const COMMIT_SELECT: &str = r#"SELECT c.id, c.conversation_id, c.incarnation, c.epoch,
        c.sender_user_id, c.sender_device_key_id, u.principal_id AS sender_principal_id,
        d.device_id AS sender_device_id, c.message, c.created_at
    FROM mls_commit c
    LEFT JOIN "user" u ON u.id = c.sender_user_id
    LEFT JOIN device_key d ON d.id = c.sender_device_key_id"#;

#[derive(FromQueryResult)]
struct CommitRow {
    id: Uuid,
    conversation_id: Uuid,
    incarnation: i32,
    epoch: i64,
    sender_user_id: Option<Uuid>,
    sender_device_key_id: Option<Uuid>,
    sender_principal_id: Option<String>,
    sender_device_id: Option<Uuid>,
    message: Vec<u8>,
    created_at: DateTimeWithTimeZone,
}

impl From<CommitRow> for MlsCommit {
    fn from(row: CommitRow) -> Self {
        Self {
            id: row.id.into(),
            conversation_id: row.conversation_id.into(),
            incarnation: row.incarnation,
            epoch: row.epoch,
            sender_user_id: row.sender_user_id.map(Into::into),
            sender_device_key_id: row.sender_device_key_id.map(Into::into),
            sender_principal_id: row.sender_principal_id,
            sender_device_id: row.sender_device_id,
            message: row.message,
            created_at: row.created_at.into(),
        }
    }
}

#[derive(FromQueryResult)]
struct WelcomeRow {
    id: Uuid,
    server_id: Uuid,
    conversation_id: Uuid,
    incarnation: i32,
    epoch: i64,
    message: Vec<u8>,
    created_at: DateTimeWithTimeZone,
    expires_at: DateTimeWithTimeZone,
}

const READD_SELECT: &str = r#"SELECT r.id, r.conversation_id, r.incarnation, r.device_key_id,
        d.user_id, COALESCE(u.principal_id, '') AS principal_id, d.device_id, r.payload,
        r.signature, r.created_at, r.fulfilled_epoch
    FROM mls_readd_request r
    JOIN device_key d ON d.id = r.device_key_id
    JOIN "user" u ON u.id = d.user_id"#;

#[derive(FromQueryResult)]
struct ReaddRow {
    id: Uuid,
    conversation_id: Uuid,
    incarnation: i32,
    device_key_id: Uuid,
    user_id: Uuid,
    principal_id: String,
    device_id: Uuid,
    payload: String,
    signature: String,
    created_at: DateTimeWithTimeZone,
    fulfilled_epoch: Option<i64>,
}

impl From<ReaddRow> for MlsReaddRequest {
    fn from(row: ReaddRow) -> Self {
        Self {
            id: row.id.into(),
            conversation_id: row.conversation_id.into(),
            incarnation: row.incarnation,
            device: MlsDevice {
                device_key_id: row.device_key_id.into(),
                user_id: row.user_id.into(),
                principal_id: row.principal_id,
                device_id: row.device_id,
            },
            payload: row.payload,
            signature: row.signature,
            created_at: row.created_at.into(),
            fulfilled_epoch: row.fulfilled_epoch,
        }
    }
}

#[derive(FromQueryResult)]
struct FrankingKeyRow {
    id: i32,
    secret: Vec<u8>,
    created_at: DateTimeWithTimeZone,
}

impl From<FrankingKeyRow> for FrankingKey {
    fn from(row: FrankingKeyRow) -> Self {
        Self {
            id: row.id,
            secret: zeroize::Zeroizing::new(row.secret),
            created_at: row.created_at.into(),
        }
    }
}

#[derive(FromQueryResult)]
struct FrankingRevisionRow {
    message_id: Uuid,
    revision: i32,
    franking_commitment: Vec<u8>,
    sent_at_us: i64,
    sender_device_id: Uuid,
    created_at: DateTimeWithTimeZone,
}

impl From<FrankingRevisionRow> for FrankingRevision {
    fn from(row: FrankingRevisionRow) -> Self {
        Self {
            message_id: row.message_id.into(),
            revision: row.revision,
            franking_commitment: row.franking_commitment,
            sent_at_us: row.sent_at_us,
            sender_device_id: row.sender_device_id,
            created_at: row.created_at.into(),
        }
    }
}

#[derive(FromQueryResult)]
struct MaxKeyIdRow {
    max_id: Option<i32>,
}

pub struct SeaOrmE2eeRepository {
    db: DbHandle,
}

impl SeaOrmE2eeRepository {
    /// A repository on `db`, a pool connection or a transaction.
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn statement(sql: &str, values: Vec<Value>) -> Statement {
        Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
    }

    async fn all<T: FromQueryResult>(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<T>> {
        T::find_by_statement(Self::statement(sql, values))
            .all(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn one<T: FromQueryResult>(
        &self,
        sql: &str,
        values: Vec<Value>,
    ) -> RepoResult<Option<T>> {
        T::find_by_statement(Self::statement(sql, values))
            .one(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        self.db
            .execute_raw(Self::statement(sql, values))
            .await
            .map(|res| res.rows_affected())
            .map_err(map_db_err)
    }

    async fn message(&self, id: Uuid) -> RepoResult<Message> {
        message::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(Into::into)
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))
    }
}

/// The envelope columns of `message`, in the order the insert and update statements bind them.
fn envelope_values(envelope: MessageEncryption) -> Vec<Value> {
    let sender: Option<Uuid> = envelope.sender_device_key_id.map(Into::into);
    vec![
        envelope.ciphertext.into(),
        envelope.incarnation.into(),
        envelope.epoch.into(),
        sender.into(),
        envelope.franking_commitment.into(),
        envelope.franking_receipt.into(),
        envelope.franking_key_id.into(),
    ]
}

#[async_trait]
impl E2eeRepository for SeaOrmE2eeRepository {
    async fn get_device(&self, device_key_id: DeviceKeyId) -> RepoResult<Option<E2eeDevice>> {
        let id: Uuid = device_key_id.into();
        let row: Option<DeviceRow> = self
            .one(
                &format!(
                    r#"SELECT {DEVICE_COLUMNS} FROM device_key d JOIN "user" u ON u.id = d.user_id
                       WHERE d.id = $1"#
                ),
                vec![id.into()],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn find_device(
        &self,
        principal_id: &str,
        device_id: Uuid,
    ) -> RepoResult<Option<E2eeDevice>> {
        let row: Option<DeviceRow> = self
            .one(
                &format!(
                    r#"SELECT {DEVICE_COLUMNS} FROM device_key d JOIN "user" u ON u.id = d.user_id
                       WHERE u.principal_id = $1 AND d.device_id = $2"#
                ),
                vec![principal_id.into(), device_id.into()],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn lock_device(&self, device_key_id: DeviceKeyId) -> RepoResult<bool> {
        let id: Uuid = device_key_id.into();
        let row = self
            .db
            .query_one_raw(Self::statement(
                "SELECT id FROM device_key WHERE id = $1 FOR UPDATE",
                vec![id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(row.is_some())
    }

    async fn touch_key_package_upload(&self, device_key_id: DeviceKeyId) -> RepoResult<()> {
        let id: Uuid = device_key_id.into();
        self.execute(
            "UPDATE device_key SET last_key_package_upload_at = now() WHERE id = $1",
            vec![id.into()],
        )
        .await?;
        Ok(())
    }

    async fn ready_users(
        &self,
        user_ids: &[UserId],
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<UserId>> {
        let rows = self
            .db
            .query_all_raw(Self::statement(
                r#"SELECT u.id FROM "user" u
                   WHERE u.id = ANY($1) AND u.principal_id IS NOT NULL
                     AND EXISTS (SELECT 1 FROM device_key d
                                 WHERE d.user_id = u.id AND d.revoked_at IS NULL
                                   AND d.last_key_package_upload_at >= $2)"#,
                vec![uuids(user_ids), utc(active_since)],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                row.try_get::<Uuid>("", "id")
                    .map(Into::into)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn mls_active_device_ids(
        &self,
        user_id: UserId,
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<Uuid>> {
        let user: Uuid = user_id.into();
        let rows = self
            .db
            .query_all_raw(Self::statement(
                "SELECT device_id FROM device_key \
                 WHERE user_id = $1 AND revoked_at IS NULL \
                   AND last_key_package_upload_at >= $2",
                vec![user.into(), utc(active_since)],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| row.try_get::<Uuid>("", "device_id").map_err(map_db_err))
            .collect()
    }

    async fn desired_devices(
        &self,
        conversation_id: ConversationId,
        active_since: DateTime<Utc>,
        cipher_suite: i32,
    ) -> RepoResult<Vec<MlsDesiredDevice>> {
        let id: Uuid = conversation_id.into();
        let rows: Vec<DesiredDeviceRow> = self
            .all(
                &format!(
                    r#"SELECT d.id AS device_key_id, d.user_id, u.principal_id, d.device_id,
                              EXISTS (SELECT 1 FROM mls_key_package k
                                      WHERE k.device_key_id = d.id AND k.cipher_suite = $3
                                        AND {}
                                        AND (k.last_resort OR k.claimed_at IS NULL))
                                  AS has_key_package
                       FROM conversation_member cm
                       JOIN "user" u ON u.id = cm.user_id
                       JOIN device_key d ON d.user_id = u.id
                       WHERE cm.conversation_id = $1 AND u.principal_id IS NOT NULL
                         AND d.revoked_at IS NULL AND d.last_key_package_upload_at >= $2
                       ORDER BY d.device_id"#,
                    usable("k.")
                ),
                vec![id.into(), utc(active_since), cipher_suite.into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| MlsDesiredDevice {
                device: MlsDevice {
                    device_key_id: row.device_key_id.into(),
                    user_id: row.user_id.into(),
                    principal_id: row.principal_id,
                    device_id: row.device_id,
                },
                has_key_package: row.has_key_package,
            })
            .collect())
    }

    async fn members_by_principal(
        &self,
        conversation_id: ConversationId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>> {
        if principal_ids.is_empty() {
            return Ok(Vec::new());
        }
        let id: Uuid = conversation_id.into();
        let rows: Vec<PrincipalUserRow> = self
            .all(
                r#"SELECT u.principal_id, u.id AS user_id
                   FROM conversation_member cm JOIN "user" u ON u.id = cm.user_id
                   WHERE cm.conversation_id = $1 AND u.principal_id = ANY($2)"#,
                vec![id.into(), principal_ids.to_vec().into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.principal_id, row.user_id.into()))
            .collect())
    }

    async fn principals_visible_to(
        &self,
        viewer: UserId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>> {
        if principal_ids.is_empty() {
            return Ok(Vec::new());
        }
        let viewer: Uuid = viewer.into();
        let rows: Vec<PrincipalUserRow> = self
            .all(
                r#"SELECT u.principal_id, u.id AS user_id FROM "user" u
                   WHERE u.principal_id = ANY($2)
                     AND (u.id = $1 OR EXISTS (
                         SELECT 1 FROM server_member a
                         JOIN server_member b ON b.server_id = a.server_id
                         WHERE a.user_id = $1 AND b.user_id = u.id))"#,
                vec![viewer.into(), principal_ids.to_vec().into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.principal_id, row.user_id.into()))
            .collect())
    }

    async fn count_available_key_packages(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<i64> {
        let id: Uuid = device_key_id.into();
        let row = self
            .db
            .query_one_raw(Self::statement(
                &format!(
                    "SELECT count(*) AS n FROM mls_key_package \
                     WHERE device_key_id = $1 AND cipher_suite = $2 AND claimed_at IS NULL \
                       AND NOT last_resort AND {}",
                    usable("")
                ),
                vec![id.into(), cipher_suite.into()],
            ))
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| internal("count returned no row"))?;
        row.try_get::<i64>("", "n").map_err(map_db_err)
    }

    async fn delete_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<u64> {
        let id: Uuid = device_key_id.into();
        self.execute(
            "DELETE FROM mls_key_package \
             WHERE device_key_id = $1 AND cipher_suite = $2 AND last_resort",
            vec![id.into(), cipher_suite.into()],
        )
        .await
    }

    async fn insert_key_package(
        &self,
        key_package: NewMlsKeyPackage,
    ) -> RepoResult<MlsKeyPackageId> {
        let id = MlsKeyPackageId::new();
        let row_id: Uuid = id.into();
        let user_id: Uuid = key_package.user_id.into();
        let device_key_id: Uuid = key_package.device_key_id.into();
        self.execute(
            "INSERT INTO mls_key_package \
             (id, user_id, device_key_id, cipher_suite, key_package_ref, key_package, \
              last_resort, not_after) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            vec![
                row_id.into(),
                user_id.into(),
                device_key_id.into(),
                key_package.cipher_suite.into(),
                key_package.key_package_ref.into(),
                key_package.key_package.into(),
                key_package.last_resort.into(),
                utc(key_package.not_after),
            ],
        )
        .await?;
        Ok(id)
    }

    async fn key_package_status(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<MlsKeyPackageStatus>> {
        let id: Uuid = device_key_id.into();
        let rows: Vec<StatusRow> = self
            .all(
                &format!(
                    "SELECT cipher_suite,
                            count(*) FILTER (WHERE NOT last_resort AND claimed_at IS NULL
                                               AND {usable}) AS available,
                            max(not_after) FILTER (WHERE last_resort) AS last_resort_not_after,
                            max(claim_count) FILTER (WHERE last_resort) AS last_resort_claims,
                            min(not_after) FILTER (WHERE NOT last_resort AND claimed_at IS NULL
                                                     AND {usable}) AS oldest_not_after
                     FROM mls_key_package WHERE device_key_id = $1
                     GROUP BY cipher_suite ORDER BY cipher_suite",
                    usable = usable("")
                ),
                vec![id.into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| MlsKeyPackageStatus {
                cipher_suite: row.cipher_suite,
                available: row.available,
                last_resort_not_after: row.last_resort_not_after.map(Into::into),
                last_resort_claims: row.last_resort_claims.unwrap_or(0),
                oldest_not_after: row.oldest_not_after.map(Into::into),
            })
            .collect())
    }

    async fn claim_one_time_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
        uploaded_after: Option<DateTime<Utc>>,
    ) -> RepoResult<Option<ClaimedKeyPackage>> {
        let device: Uuid = device_key_id.into();
        let claimer: Uuid = claimed_by.into();
        let conversation: Uuid = conversation_id.into();
        let order = if uploaded_after.is_some() {
            "created_at DESC, id DESC"
        } else {
            "created_at, id"
        };
        let row: Option<ClaimedRow> = self
            .one(
                &format!(
                    "UPDATE mls_key_package
                     SET claimed_at = now(), claimed_by_device_key_id = $3,
                         claimed_for_conversation_id = $4, claimed_for_incarnation = $5
                     WHERE id = (
                         SELECT id FROM mls_key_package
                         WHERE device_key_id = $1 AND cipher_suite = $2 AND claimed_at IS NULL
                           AND NOT last_resort AND {}
                           AND ($6::timestamptz IS NULL OR created_at > $6)
                         ORDER BY {order}
                         LIMIT 1
                         FOR UPDATE SKIP LOCKED)
                     RETURNING {CLAIMED_COLUMNS}",
                    usable("")
                ),
                vec![
                    device.into(),
                    cipher_suite.into(),
                    claimer.into(),
                    conversation.into(),
                    incarnation.into(),
                    uploaded_after.into(),
                ],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn claim_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Option<ClaimedKeyPackage>> {
        let device: Uuid = device_key_id.into();
        let claimer: Uuid = claimed_by.into();
        let conversation: Uuid = conversation_id.into();
        let row: Option<ClaimedRow> = self
            .one(
                &format!(
                    "UPDATE mls_key_package
                     SET claimed_at = now(), claimed_by_device_key_id = $3,
                         claimed_for_conversation_id = $4, claimed_for_incarnation = $5,
                         claim_count = claim_count + 1
                     WHERE device_key_id = $1 AND cipher_suite = $2 AND last_resort AND {}
                     RETURNING {CLAIMED_COLUMNS}",
                    usable("")
                ),
                vec![
                    device.into(),
                    cipher_suite.into(),
                    claimer.into(),
                    conversation.into(),
                    incarnation.into(),
                ],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn find_key_packages_by_refs(&self, refs: &[Vec<u8>]) -> RepoResult<Vec<MlsKeyPackage>> {
        let mut found = Vec::with_capacity(refs.len());
        for key_package_ref in refs {
            let row: Option<KeyPackageRow> = self
                .one(
                    &format!(
                        "SELECT {KEY_PACKAGE_COLUMNS} FROM mls_key_package \
                         WHERE key_package_ref = $1"
                    ),
                    vec![key_package_ref.clone().into()],
                )
                .await?;
            found.extend(row.map(MlsKeyPackage::from));
        }
        Ok(found)
    }

    async fn consume_key_packages(&self, ids: &[MlsKeyPackageId]) -> RepoResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.execute(
            "DELETE FROM mls_key_package WHERE id = ANY($1) AND NOT last_resort",
            vec![uuids(ids)],
        )
        .await
    }

    async fn current_group(&self, conversation_id: ConversationId) -> RepoResult<Option<MlsGroup>> {
        let id: Uuid = conversation_id.into();
        let row: Option<GroupRow> = self
            .one(
                &format!(
                    "SELECT {GROUP_COLUMNS} FROM mls_group \
                     WHERE conversation_id = $1 AND retired_at IS NULL"
                ),
                vec![id.into()],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn lock_current_group(
        &self,
        conversation_id: ConversationId,
        lock: GroupLock,
    ) -> RepoResult<Option<MlsGroup>> {
        let id: Uuid = conversation_id.into();
        let mode = match lock {
            GroupLock::Update => "FOR UPDATE",
            GroupLock::Share => "FOR SHARE",
        };
        let row: Option<GroupRow> = self
            .one(
                &format!(
                    "SELECT {GROUP_COLUMNS} FROM mls_group \
                     WHERE conversation_id = $1 AND retired_at IS NULL {mode}"
                ),
                vec![id.into()],
            )
            .await?;
        Ok(row.map(Into::into))
    }

    async fn insert_group(&self, group: NewMlsGroup) -> RepoResult<MlsGroup> {
        let conversation: Uuid = group.conversation_id.into();
        let creator: Option<Uuid> = group.created_by_device_key_id.map(Into::into);
        let row: Option<GroupRow> = self
            .one(
                &format!(
                    "INSERT INTO mls_group \
                     (conversation_id, incarnation, group_id, cipher_suite, epoch, \
                      created_by_device_key_id) \
                     VALUES ($1, $2, $3, $4, $5, $6) RETURNING {GROUP_COLUMNS}"
                ),
                vec![
                    conversation.into(),
                    group.incarnation.into(),
                    group.group_id.into(),
                    group.cipher_suite.into(),
                    group.epoch.into(),
                    creator.into(),
                ],
            )
            .await?;
        row.map(Into::into)
            .ok_or_else(|| internal("insert returned no row"))
    }

    async fn retire_group(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<()> {
        let id: Uuid = conversation_id.into();
        self.execute(
            "UPDATE mls_group SET retired_at = now() \
             WHERE conversation_id = $1 AND incarnation = $2 AND retired_at IS NULL",
            vec![id.into(), incarnation.into()],
        )
        .await?;
        Ok(())
    }

    async fn advance_epoch(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<i64> {
        let id: Uuid = conversation_id.into();
        let row = self
            .db
            .query_one_raw(Self::statement(
                "UPDATE mls_group SET epoch = epoch + 1 \
                 WHERE conversation_id = $1 AND incarnation = $2 RETURNING epoch",
                vec![id.into(), incarnation.into()],
            ))
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        row.try_get::<i64>("", "epoch").map_err(map_db_err)
    }

    async fn group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Vec<MlsGroupDevice>> {
        let id: Uuid = conversation_id.into();
        let rows: Vec<GroupDeviceRow> = self
            .all(
                r#"SELECT g.device_key_id, g.user_id, COALESCE(u.principal_id, '') AS principal_id,
                          d.device_id, g.added_epoch
                   FROM mls_group_device g
                   JOIN device_key d ON d.id = g.device_key_id
                   JOIN "user" u ON u.id = g.user_id
                   WHERE g.conversation_id = $1 AND g.incarnation = $2
                   ORDER BY d.device_id"#,
                vec![id.into(), incarnation.into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| MlsGroupDevice {
                device: MlsDevice {
                    device_key_id: row.device_key_id.into(),
                    user_id: row.user_id.into(),
                    principal_id: row.principal_id,
                    device_id: row.device_id,
                },
                added_epoch: row.added_epoch,
            })
            .collect())
    }

    async fn add_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        added_epoch: i64,
        devices: &[(DeviceKeyId, UserId)],
    ) -> RepoResult<()> {
        if devices.is_empty() {
            return Ok(());
        }
        let id: Uuid = conversation_id.into();
        let device_ids: Vec<DeviceKeyId> = devices.iter().map(|(d, _)| *d).collect();
        let user_ids: Vec<UserId> = devices.iter().map(|(_, u)| *u).collect();
        self.execute(
            "INSERT INTO mls_group_device \
             (conversation_id, incarnation, device_key_id, user_id, added_epoch) \
             SELECT $1, $2, x.device_key_id, x.user_id, $3 \
             FROM unnest($4::uuid[], $5::uuid[]) AS x(device_key_id, user_id)",
            vec![
                id.into(),
                incarnation.into(),
                added_epoch.into(),
                uuids(&device_ids),
                uuids(&user_ids),
            ],
        )
        .await?;
        Ok(())
    }

    async fn remove_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        device_key_ids: &[DeviceKeyId],
    ) -> RepoResult<()> {
        if device_key_ids.is_empty() {
            return Ok(());
        }
        let id: Uuid = conversation_id.into();
        self.execute(
            "DELETE FROM mls_group_device \
             WHERE conversation_id = $1 AND incarnation = $2 AND device_key_id = ANY($3)",
            vec![id.into(), incarnation.into(), uuids(device_key_ids)],
        )
        .await?;
        Ok(())
    }

    async fn mark_conversation_encrypted(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<bool> {
        let id: Uuid = conversation_id.into();
        let changed = self
            .execute(
                "UPDATE conversation SET encrypted_at = now() \
                 WHERE id = $1 AND encrypted_at IS NULL",
                vec![id.into()],
            )
            .await?;
        Ok(changed > 0)
    }

    async fn conversations_with_group_device(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<ConversationId>> {
        let id: Uuid = device_key_id.into();
        let rows = self
            .db
            .query_all_raw(Self::statement(
                "SELECT g.conversation_id FROM mls_group_device g \
                 JOIN mls_group m ON m.conversation_id = g.conversation_id \
                   AND m.incarnation = g.incarnation \
                 WHERE g.device_key_id = $1 AND m.retired_at IS NULL",
                vec![id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                row.try_get::<Uuid>("", "conversation_id")
                    .map(Into::into)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn grouped_conversations_of_user(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>> {
        let id: Uuid = user_id.into();
        let rows = self
            .db
            .query_all_raw(Self::statement(
                "SELECT m.conversation_id FROM mls_group m \
                 JOIN conversation_member cm ON cm.conversation_id = m.conversation_id \
                 WHERE cm.user_id = $1 AND m.retired_at IS NULL",
                vec![id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                row.try_get::<Uuid>("", "conversation_id")
                    .map(Into::into)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn insert_commit(&self, commit: NewMlsCommit) -> RepoResult<MlsCommit> {
        let id: Uuid = MlsCommitId::new().into();
        let conversation: Uuid = commit.conversation_id.into();
        let sender: Uuid = commit.sender_user_id.into();
        let device: Uuid = commit.sender_device_key_id.into();
        self.execute(
            "INSERT INTO mls_commit \
             (id, conversation_id, incarnation, epoch, sender_user_id, sender_device_key_id, \
              message) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            vec![
                id.into(),
                conversation.into(),
                commit.incarnation.into(),
                commit.epoch.into(),
                sender.into(),
                device.into(),
                commit.message.into(),
            ],
        )
        .await?;
        let row: Option<CommitRow> = self
            .one(&format!("{COMMIT_SELECT} WHERE c.id = $1"), vec![id.into()])
            .await?;
        row.map(Into::into)
            .ok_or_else(|| internal("inserted commit not found"))
    }

    async fn list_commits(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        after_epoch: Option<i64>,
        limit: u32,
    ) -> RepoResult<Vec<MlsCommit>> {
        let id: Uuid = conversation_id.into();
        let rows: Vec<CommitRow> = self
            .all(
                &format!(
                    "{COMMIT_SELECT}
                     WHERE c.conversation_id = $1 AND c.incarnation = $2
                       AND ($3::bigint IS NULL OR c.epoch > $3)
                     ORDER BY c.epoch
                     LIMIT $4"
                ),
                vec![
                    id.into(),
                    incarnation.into(),
                    after_epoch.into(),
                    i64::from(limit).into(),
                ],
            )
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn commit_accepted_since(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let id: Uuid = conversation_id.into();
        let row = self
            .db
            .query_one_raw(Self::statement(
                "SELECT EXISTS (SELECT 1 FROM mls_commit \
                 WHERE conversation_id = $1 AND incarnation = $2 AND created_at > $3) AS found",
                vec![id.into(), incarnation.into(), utc(since)],
            ))
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| internal("exists returned no row"))?;
        row.try_get::<bool>("", "found").map_err(map_db_err)
    }

    async fn insert_welcome(&self, welcome: NewMlsWelcome) -> RepoResult<MlsWelcomeId> {
        let id = MlsWelcomeId::new();
        let row_id: Uuid = id.into();
        let commit: Uuid = welcome.commit_id.into();
        let conversation: Uuid = welcome.conversation_id.into();
        self.execute(
            "INSERT INTO mls_welcome \
             (id, commit_id, conversation_id, incarnation, epoch, message, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            vec![
                row_id.into(),
                commit.into(),
                conversation.into(),
                welcome.incarnation.into(),
                welcome.epoch.into(),
                welcome.message.into(),
                utc(welcome.expires_at),
            ],
        )
        .await?;
        let devices: Vec<DeviceKeyId> = welcome.recipients.iter().map(|(d, _)| *d).collect();
        let users: Vec<UserId> = welcome.recipients.iter().map(|(_, u)| *u).collect();
        self.execute(
            "INSERT INTO mls_welcome_recipient (welcome_id, device_key_id, user_id) \
             SELECT $1, x.device_key_id, x.user_id \
             FROM unnest($2::uuid[], $3::uuid[]) AS x(device_key_id, user_id)",
            vec![row_id.into(), uuids(&devices), uuids(&users)],
        )
        .await?;
        Ok(id)
    }

    async fn pending_welcomes(&self, device_key_id: DeviceKeyId) -> RepoResult<Vec<MlsWelcome>> {
        let id: Uuid = device_key_id.into();
        let rows: Vec<WelcomeRow> = self
            .all(
                "SELECT w.id, c.server_id, w.conversation_id, w.incarnation, w.epoch, w.message,
                        w.created_at, w.expires_at
                 FROM mls_welcome_recipient r
                 JOIN mls_welcome w ON w.id = r.welcome_id
                 JOIN conversation c ON c.id = w.conversation_id
                 WHERE r.device_key_id = $1 AND r.acked_at IS NULL AND w.expires_at > now()
                   AND EXISTS (SELECT 1 FROM conversation_member cm
                               WHERE cm.conversation_id = w.conversation_id
                                 AND cm.user_id = r.user_id)
                 ORDER BY w.created_at, w.id",
                vec![id.into()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| MlsWelcome {
                id: row.id.into(),
                server_id: row.server_id.into(),
                conversation_id: row.conversation_id.into(),
                incarnation: row.incarnation,
                epoch: row.epoch,
                message: row.message,
                created_at: row.created_at.into(),
                expires_at: row.expires_at.into(),
            })
            .collect())
    }

    async fn ack_welcomes(
        &self,
        device_key_id: DeviceKeyId,
        ids: &[MlsWelcomeId],
    ) -> RepoResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let id: Uuid = device_key_id.into();
        self.execute(
            "UPDATE mls_welcome_recipient SET acked_at = now() \
             WHERE device_key_id = $1 AND welcome_id = ANY($2) AND acked_at IS NULL",
            vec![id.into(), uuids(ids)],
        )
        .await
    }

    async fn insert_readd_request(
        &self,
        request: NewMlsReaddRequest,
    ) -> RepoResult<MlsReaddRequest> {
        let id: Uuid = MlsReaddRequestId::new().into();
        let conversation: Uuid = request.conversation_id.into();
        let device: Uuid = request.device_key_id.into();
        let inserted = self
            .execute(
                "WITH nonce AS ( \
                   INSERT INTO mls_readd_nonce (conversation_id, incarnation, device_key_id, nonce) \
                   VALUES ($2, $3, $4, $7) ON CONFLICT DO NOTHING RETURNING 1) \
                 INSERT INTO mls_readd_request \
                 (id, conversation_id, incarnation, device_key_id, payload, signature) \
                 SELECT $1, $2, $3, $4, $5, $6 FROM nonce",
                vec![
                    id.into(),
                    conversation.into(),
                    request.incarnation.into(),
                    device.into(),
                    request.payload.into(),
                    request.signature.into(),
                    request.nonce.into(),
                ],
            )
            .await?;
        if inserted == 0 {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let row: Option<ReaddRow> = self
            .one(&format!("{READD_SELECT} WHERE r.id = $1"), vec![id.into()])
            .await?;
        row.map(Into::into)
            .ok_or_else(|| internal("inserted re-add request not found"))
    }

    async fn open_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<Vec<MlsReaddRequest>> {
        let id: Uuid = conversation_id.into();
        let rows: Vec<ReaddRow> = self
            .all(
                &format!(
                    "{READD_SELECT}
                     WHERE r.conversation_id = $1 AND r.incarnation = $2
                       AND r.fulfilled_epoch IS NULL AND r.created_at >= $3
                     ORDER BY r.created_at, r.id"
                ),
                vec![id.into(), incarnation.into(), utc(since)],
            )
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn fulfill_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        device_key_ids: &[DeviceKeyId],
        epoch: i64,
    ) -> RepoResult<u64> {
        if device_key_ids.is_empty() {
            return Ok(0);
        }
        let id: Uuid = conversation_id.into();
        self.execute(
            "UPDATE mls_readd_request SET fulfilled_epoch = $4 \
             WHERE conversation_id = $1 AND incarnation = $2 AND device_key_id = ANY($3) \
               AND fulfilled_epoch IS NULL",
            vec![
                id.into(),
                incarnation.into(),
                uuids(device_key_ids),
                epoch.into(),
            ],
        )
        .await
    }

    async fn insert_encrypted_message(&self, message: NewEncryptedMessage) -> RepoResult<Message> {
        let id: Uuid = message.id.into();
        let conversation: Uuid = message.conversation_id.into();
        let thread: Option<Uuid> = message.thread_id.map(Into::into);
        let forwarded: Option<Uuid> = message.forwarded_from_message_id.map(Into::into);
        let sender: Uuid = message.sent_by.into();
        let mut values = vec![
            id.into(),
            conversation.into(),
            thread.into(),
            forwarded.into(),
            utc(message.sent_at),
            sender.into(),
        ];
        values.extend(envelope_values(message.envelope));
        self.execute(
            "INSERT INTO message \
             (id, conversation_id, thread_id, forwarded_from_message_id, sent_at, sent_by, \
              text, is_deleted, has_attachments, encrypted, mls_ciphertext, mls_incarnation, \
              mls_epoch, sender_device_key_id, franking_commitment, franking_receipt, \
              franking_key_id) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, false, false, true, \
                     $7, $8, $9, $10, $11, $12, $13)",
            values,
        )
        .await?;
        self.message(id).await
    }

    async fn replace_envelope(
        &self,
        message_id: MessageId,
        envelope: MessageEncryption,
    ) -> RepoResult<Message> {
        let id: Uuid = message_id.into();
        let mut values = vec![id.into()];
        values.extend(envelope_values(envelope));
        let changed = self
            .execute(
                "UPDATE message SET mls_ciphertext = $2, mls_incarnation = $3, mls_epoch = $4, \
                   sender_device_key_id = $5, franking_commitment = $6, \
                   franking_receipt = $7, franking_key_id = $8 \
                 WHERE id = $1 AND encrypted",
                values,
            )
            .await?;
        if changed == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        self.message(id).await
    }

    async fn append_franking_revision(&self, revision: NewFrankingRevision) -> RepoResult<()> {
        let message: Uuid = revision.message_id.into();
        self.execute(
            "INSERT INTO message_franking_revision \
             (message_id, revision, franking_commitment, sent_at_us, sender_device_id) \
             SELECT $1, COALESCE(max(revision) + 1, 0), $2, $3, $4 \
             FROM message_franking_revision WHERE message_id = $1",
            vec![
                message.into(),
                revision.franking_commitment.into(),
                revision.sent_at_us.into(),
                revision.sender_device_id.into(),
            ],
        )
        .await?;
        Ok(())
    }

    async fn franking_revisions(&self, message_id: MessageId) -> RepoResult<Vec<FrankingRevision>> {
        let message: Uuid = message_id.into();
        let rows: Vec<FrankingRevisionRow> = self
            .all(
                "SELECT message_id, revision, franking_commitment, sent_at_us, sender_device_id, \
                        created_at \
                 FROM message_franking_revision WHERE message_id = $1 ORDER BY revision",
                vec![message.into()],
            )
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn legacy_franking_keys(&self) -> RepoResult<Vec<FrankingKey>> {
        let rows: Vec<FrankingKeyRow> = self
            .all(
                "SELECT id, secret, created_at FROM franking_key ORDER BY id",
                vec![],
            )
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn max_message_franking_key_id(&self) -> RepoResult<Option<i32>> {
        let row: Option<MaxKeyIdRow> = self
            .one(
                "SELECT max(franking_key_id) AS max_id FROM message WHERE encrypted",
                vec![],
            )
            .await?;
        Ok(row.and_then(|r| r.max_id))
    }

    async fn purge(&self, retention: MlsRetention) -> RepoResult<MlsPurged> {
        let commits = self
            .execute(
                "DELETE FROM mls_commit WHERE created_at < now() - make_interval(secs => $1)",
                vec![seconds(retention.commits)],
            )
            .await?;
        let welcomes = self
            .execute("DELETE FROM mls_welcome WHERE expires_at <= now()", vec![])
            .await?;
        let key_packages = self
            .execute(
                "DELETE FROM mls_key_package WHERE not_after <= now() \
                 OR (NOT last_resort AND claimed_at < now() - make_interval(secs => $1))",
                vec![seconds(retention.claimed_key_packages)],
            )
            .await?;
        let readd_requests = self
            .execute(
                "DELETE FROM mls_readd_request WHERE fulfilled_epoch IS NOT NULL \
                 OR created_at < now() - make_interval(secs => $1)",
                vec![seconds(retention.readd_requests)],
            )
            .await?;
        let readd_nonces = self
            .execute(
                "DELETE FROM mls_readd_nonce WHERE created_at < now() - make_interval(secs => $1)",
                vec![seconds(retention.readd_nonces)],
            )
            .await?;
        let franked_contents = self
            .execute(
                "UPDATE message_report SET franked_content = NULL \
                 WHERE franked_content IS NOT NULL \
                   AND resolved_at < now() - make_interval(secs => $1)",
                vec![seconds(retention.franked_content)],
            )
            .await?;
        Ok(MlsPurged {
            commits,
            welcomes,
            key_packages,
            readd_requests,
            readd_nonces,
            franked_contents,
        })
    }
}
