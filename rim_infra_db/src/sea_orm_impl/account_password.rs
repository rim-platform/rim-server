use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::account_password::model::{
    AccountPassword, AttemptSchedule, AttemptStart, PasswordLogin, PasswordLoginId,
    PasswordLoginPurpose, PasswordReset, PasswordResetId,
};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
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

fn utc(at: DateTime<Utc>) -> Value {
    DateTimeWithTimeZone::from(at).into()
}

fn count_of(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

const PASSWORD_COLUMNS: &str = "user_id, registration, share_sealed, failed_attempts, \
     next_attempt_at, locked_at, created_at, updated_at";

#[derive(FromQueryResult)]
struct PasswordRow {
    user_id: Uuid,
    registration: Vec<u8>,
    share_sealed: Vec<u8>,
    failed_attempts: i32,
    next_attempt_at: Option<DateTimeWithTimeZone>,
    locked_at: Option<DateTimeWithTimeZone>,
    created_at: DateTimeWithTimeZone,
    updated_at: DateTimeWithTimeZone,
}

impl From<PasswordRow> for AccountPassword {
    fn from(row: PasswordRow) -> Self {
        Self {
            user_id: row.user_id.into(),
            registration: row.registration,
            share_sealed: row.share_sealed,
            failed_attempts: count_of(row.failed_attempts),
            next_attempt_at: row.next_attempt_at.map(Into::into),
            locked_at: row.locked_at.map(Into::into),
            created_at: row.created_at.into(),
            updated_at: row.updated_at.into(),
        }
    }
}

#[derive(FromQueryResult)]
struct CountedRow {
    registration: Vec<u8>,
    failed_attempts: i32,
}

#[derive(FromQueryResult)]
struct RefusedRow {
    next_attempt_at: Option<DateTimeWithTimeZone>,
    locked_at: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct LoginRow {
    id: Uuid,
    user_id: Option<Uuid>,
    purpose: String,
    state: Vec<u8>,
    expires_at: DateTimeWithTimeZone,
}

#[derive(FromQueryResult)]
struct ResetRow {
    id: Uuid,
    user_id: Uuid,
    expires_at: DateTimeWithTimeZone,
}

#[derive(FromQueryResult)]
struct RewrapRow {
    rewrap: Vec<u8>,
}

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

/// Account passwords on `account_password`, login states on `password_login`, rewraps on
/// `password_rewrap` and resets on `password_reset`, in raw SQL.
pub struct SeaOrmAccountPasswordRepository {
    db: DbHandle,
}

impl SeaOrmAccountPasswordRepository {
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
        Ok(self
            .db
            .execute_raw(statement(sql, values))
            .await
            .map_err(map_db_err)?
            .rows_affected())
    }
}

#[async_trait]
impl AccountPasswordRepository for SeaOrmAccountPasswordRepository {
    async fn get(&self, user_id: UserId) -> RepoResult<Option<AccountPassword>> {
        Ok(self
            .one::<PasswordRow>(
                &format!("SELECT {PASSWORD_COLUMNS} FROM account_password WHERE user_id = $1"),
                vec![uuid(user_id)],
            )
            .await?
            .map(Into::into))
    }

    async fn insert(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        share_sealed: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.execute(
            "INSERT INTO account_password (user_id, registration, share_sealed, created_at, \
             updated_at) VALUES ($1, $2, $3, $4, $4)",
            vec![
                uuid(user_id),
                registration.into(),
                share_sealed.into(),
                utc(now),
            ],
        )
        .await?;
        Ok(())
    }

    async fn replace_registration(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let updated = self
            .execute(
                "UPDATE account_password SET registration = $2, failed_attempts = 0, \
                 next_attempt_at = NULL, locked_at = NULL, updated_at = $3 WHERE user_id = $1",
                vec![uuid(user_id), registration.into(), utc(now)],
            )
            .await?;
        Ok(updated > 0)
    }

    async fn count_passwords(&self) -> RepoResult<u64> {
        let row: Option<CountRow> = self
            .one("SELECT count(*) AS count FROM account_password", vec![])
            .await?;
        Ok(row.map_or(0, |r| u64::try_from(r.count).unwrap_or(0)))
    }

    async fn start_attempt(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
        schedule: &AttemptSchedule,
    ) -> RepoResult<AttemptStart> {
        // `failed_attempts` on the right-hand side is the count before this try; the wait is
        // the schedule's entry for the new count, or its last entry past the end.
        let counted: Option<CountedRow> = self
            .one(
                "UPDATE account_password SET \
                     failed_attempts = failed_attempts + 1, \
                     next_attempt_at = CASE \
                         WHEN failed_attempts + 1 >= $4 THEN NULL \
                         WHEN COALESCE(($3::int8[])[LEAST(failed_attempts + 1, \
                             cardinality($3::int8[]))], 0) > 0 \
                         THEN $2 + make_interval(secs => ($3::int8[])[LEAST(failed_attempts + 1, \
                             cardinality($3::int8[]))]) \
                         ELSE NULL END, \
                     locked_at = CASE WHEN failed_attempts + 1 >= $4 THEN $2 ELSE NULL END, \
                     updated_at = $2 \
                 WHERE user_id = $1 AND locked_at IS NULL \
                     AND (next_attempt_at IS NULL OR next_attempt_at <= $2) \
                 RETURNING registration, failed_attempts",
                vec![
                    uuid(user_id),
                    utc(now),
                    schedule.waits_seconds.clone().into(),
                    i32::try_from(schedule.lock_after)
                        .unwrap_or(i32::MAX)
                        .into(),
                ],
            )
            .await?;
        if let Some(row) = counted {
            return Ok(AttemptStart::Counted {
                registration: row.registration,
                failed_attempts: count_of(row.failed_attempts),
            });
        }
        let refused: Option<RefusedRow> = self
            .one(
                "SELECT next_attempt_at, locked_at FROM account_password WHERE user_id = $1",
                vec![uuid(user_id)],
            )
            .await?;
        Ok(match refused {
            None => AttemptStart::NoPassword,
            Some(row) if row.locked_at.is_some() => AttemptStart::Locked,
            // A success may clear the wait between the two statements; the caller then tries
            // again at once.
            Some(row) => AttemptStart::Wait {
                until: row.next_attempt_at.map_or(now, Into::into),
            },
        })
    }

    async fn clear_attempts(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<()> {
        self.execute(
            "UPDATE account_password SET failed_attempts = 0, next_attempt_at = NULL, \
             locked_at = NULL, updated_at = $2 WHERE user_id = $1",
            vec![uuid(user_id), utc(now)],
        )
        .await?;
        Ok(())
    }

    async fn insert_login(&self, login: PasswordLogin, now: DateTime<Utc>) -> RepoResult<()> {
        self.execute(
            "DELETE FROM password_login WHERE expires_at <= $1",
            vec![utc(now)],
        )
        .await?;
        self.execute(
            "INSERT INTO password_login (id, user_id, purpose, state, expires_at) \
             VALUES ($1, $2, $3, $4, $5)",
            vec![
                uuid(login.id),
                login.user_id.map(Uuid::from).into(),
                login.purpose.as_str().into(),
                login.state.into(),
                utc(login.expires_at),
            ],
        )
        .await?;
        Ok(())
    }

    async fn take_login(&self, id: PasswordLoginId) -> RepoResult<Option<PasswordLogin>> {
        let row: Option<LoginRow> = self
            .one(
                "DELETE FROM password_login WHERE id = $1 \
                 RETURNING id, user_id, purpose, state, expires_at",
                vec![uuid(id)],
            )
            .await?;
        row.map(|row| {
            Ok(PasswordLogin {
                id: row.id.into(),
                user_id: row.user_id.map(Into::into),
                purpose: PasswordLoginPurpose::parse(&row.purpose)
                    .ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData))?,
                state: row.state,
                expires_at: row.expires_at.into(),
            })
        })
        .transpose()
    }

    async fn insert_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.execute(
            "DELETE FROM password_ticket WHERE expires_at <= $1",
            vec![utc(now)],
        )
        .await?;
        self.execute(
            "INSERT INTO password_ticket (token_hash, user_id, expires_at) VALUES ($1, $2, $3)",
            vec![token_hash.into(), uuid(user_id), utc(expires_at)],
        )
        .await?;
        Ok(())
    }

    async fn consume_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let deleted = self
            .execute(
                "DELETE FROM password_ticket WHERE token_hash = $1 AND user_id = $2 \
                 AND expires_at > $3",
                vec![token_hash.into(), uuid(user_id), utc(now)],
            )
            .await?;
        Ok(deleted > 0)
    }

    async fn insert_rewrap(
        &self,
        user_id: UserId,
        rewrap: Vec<u8>,
        now: DateTime<Utc>,
        keep: u32,
    ) -> RepoResult<()> {
        self.execute(
            "INSERT INTO password_rewrap (id, user_id, rewrap, created_at) VALUES ($1, $2, $3, $4)",
            vec![uuid(Uuid::now_v7()), uuid(user_id), rewrap.into(), utc(now)],
        )
        .await?;
        self.execute(
            "DELETE FROM password_rewrap WHERE user_id = $1 AND id NOT IN \
             (SELECT id FROM password_rewrap WHERE user_id = $1 \
              ORDER BY created_at DESC, id DESC LIMIT $2)",
            vec![uuid(user_id), i64::from(keep).into()],
        )
        .await?;
        Ok(())
    }

    async fn list_rewraps(&self, user_id: UserId) -> RepoResult<Vec<Vec<u8>>> {
        Ok(self
            .all::<RewrapRow>(
                "SELECT rewrap FROM password_rewrap WHERE user_id = $1 \
                 ORDER BY created_at DESC, id DESC",
                vec![uuid(user_id)],
            )
            .await?
            .into_iter()
            .map(|r| r.rewrap)
            .collect())
    }

    async fn delete_rewraps(&self, user_id: UserId) -> RepoResult<u64> {
        self.execute(
            "DELETE FROM password_rewrap WHERE user_id = $1",
            vec![uuid(user_id)],
        )
        .await
    }

    async fn insert_reset(&self, reset: PasswordReset, now: DateTime<Utc>) -> RepoResult<()> {
        self.execute(
            "DELETE FROM password_reset WHERE expires_at <= $1",
            vec![utc(now)],
        )
        .await?;
        self.execute(
            "INSERT INTO password_reset (id, user_id, expires_at) VALUES ($1, $2, $3)",
            vec![uuid(reset.id), uuid(reset.user_id), utc(reset.expires_at)],
        )
        .await?;
        Ok(())
    }

    async fn take_reset(&self, id: PasswordResetId) -> RepoResult<Option<PasswordReset>> {
        Ok(self
            .one::<ResetRow>(
                "DELETE FROM password_reset WHERE id = $1 RETURNING id, user_id, expires_at",
                vec![uuid(id)],
            )
            .await?
            .map(|row| PasswordReset {
                id: row.id.into(),
                user_id: row.user_id.into(),
                expires_at: row.expires_at.into(),
            }))
    }
}
