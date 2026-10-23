use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rim_core::public_join_link::model::{
    JoinRequest, JoinRequestId, JoinRequestStatus, NewJoinRequest, PublicJoinLink, PublicLinkMode,
};
use rim_core::public_join_link::repository::{
    JoinRequestCursor, ListJoinRequestsParams, PublicJoinLinkRepository,
};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

/// How long a join through a link is kept for the link's join counts.
const JOIN_RECORD_DAYS: i64 = 7;

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

fn invalid() -> RepoError {
    RepoError::new(RepoErrorKind::InvalidData)
}

const LINK_COLUMNS: &str = "server_id, token_hash, sealed_token, enabled, mode, role, \
     allow_remote, enabled_at, issued_at, issued_by_user_id";

const REQUEST_COLUMNS: &str = "id, server_id, user_id, message, status, created_at, expires_at, \
     decided_at, decided_by_user_id";

#[derive(FromQueryResult)]
struct LinkRow {
    server_id: Uuid,
    token_hash: String,
    sealed_token: Vec<u8>,
    enabled: bool,
    mode: String,
    role: String,
    allow_remote: bool,
    enabled_at: Option<DateTimeWithTimeZone>,
    issued_at: DateTimeWithTimeZone,
    issued_by_user_id: Option<Uuid>,
}

impl TryFrom<LinkRow> for PublicJoinLink {
    type Error = RepoError;

    fn try_from(row: LinkRow) -> RepoResult<Self> {
        Ok(Self {
            server_id: row.server_id.into(),
            token_hash: row.token_hash,
            sealed_token: row.sealed_token,
            enabled: row.enabled,
            mode: PublicLinkMode::parse(&row.mode).ok_or_else(invalid)?,
            role: ServerMemberRole::from_str(&row.role).map_err(|_| invalid())?,
            allow_remote: row.allow_remote,
            enabled_at: row.enabled_at.map(Into::into),
            issued_at: row.issued_at.into(),
            issued_by: row.issued_by_user_id.map(Into::into),
        })
    }
}

#[derive(FromQueryResult)]
struct RequestRow {
    id: Uuid,
    server_id: Uuid,
    user_id: Uuid,
    message: Option<String>,
    status: String,
    created_at: DateTimeWithTimeZone,
    expires_at: DateTimeWithTimeZone,
    decided_at: Option<DateTimeWithTimeZone>,
    decided_by_user_id: Option<Uuid>,
}

impl TryFrom<RequestRow> for JoinRequest {
    type Error = RepoError;

    fn try_from(row: RequestRow) -> RepoResult<Self> {
        Ok(Self {
            id: row.id.into(),
            server_id: row.server_id.into(),
            user_id: row.user_id.into(),
            message: row.message,
            status: JoinRequestStatus::parse(&row.status).ok_or_else(invalid)?,
            created_at: row.created_at.into(),
            expires_at: row.expires_at.into(),
            decided_at: row.decided_at.map(Into::into),
            decided_by: row.decided_by_user_id.map(Into::into),
        })
    }
}

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(FromQueryResult)]
struct AtRow {
    at: Option<DateTimeWithTimeZone>,
}

/// Public links on `workspace_public_link`, their joins on `workspace_public_link_join` and
/// join requests on `workspace_join_request`, in raw SQL.
pub struct SeaOrmPublicJoinLinkRepository {
    db: DbHandle,
}

impl SeaOrmPublicJoinLinkRepository {
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

    async fn count(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        let row: Option<CountRow> = self.one(sql, values).await?;
        Ok(row.map_or(0, |r| u64::try_from(r.count).unwrap_or(0)))
    }

    async fn links(&self, sql: &str, values: Vec<Value>) -> RepoResult<Option<PublicJoinLink>> {
        self.one::<LinkRow>(sql, values)
            .await?
            .map(TryInto::try_into)
            .transpose()
    }

    async fn requests(&self, sql: &str, values: Vec<Value>) -> RepoResult<Vec<JoinRequest>> {
        self.all::<RequestRow>(sql, values)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }
}

#[async_trait]
impl PublicJoinLinkRepository for SeaOrmPublicJoinLinkRepository {
    async fn link(&self, server_id: ServerId) -> RepoResult<Option<PublicJoinLink>> {
        self.links(
            &format!("SELECT {LINK_COLUMNS} FROM workspace_public_link WHERE server_id = $1"),
            vec![uuid(server_id)],
        )
        .await
    }

    async fn link_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PublicJoinLink>> {
        self.links(
            &format!("SELECT {LINK_COLUMNS} FROM workspace_public_link WHERE token_hash = $1"),
            vec![token_hash.into()],
        )
        .await
    }

    async fn save_link(&self, link: PublicJoinLink) -> RepoResult<PublicJoinLink> {
        let sql = format!(
            "INSERT INTO workspace_public_link ({LINK_COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (server_id) DO UPDATE SET token_hash = EXCLUDED.token_hash, \
                 sealed_token = EXCLUDED.sealed_token, enabled = EXCLUDED.enabled, \
                 mode = EXCLUDED.mode, role = EXCLUDED.role, \
                 allow_remote = EXCLUDED.allow_remote, enabled_at = EXCLUDED.enabled_at, \
                 issued_at = EXCLUDED.issued_at, \
                 issued_by_user_id = EXCLUDED.issued_by_user_id, updated_at = now() \
             RETURNING {LINK_COLUMNS}"
        );
        self.links(
            &sql,
            vec![
                uuid(link.server_id),
                link.token_hash.into(),
                link.sealed_token.into(),
                link.enabled.into(),
                link.mode.as_str().into(),
                link.role.as_str().into(),
                link.allow_remote.into(),
                link.enabled_at.map(DateTimeWithTimeZone::from).into(),
                utc(link.issued_at),
                link.issued_by.map(Uuid::from).into(),
            ],
        )
        .await?
        .ok_or_else(|| RepoError::new(RepoErrorKind::Internal))
    }

    async fn count_links(&self) -> RepoResult<u64> {
        self.count(
            "SELECT count(*) AS count FROM workspace_public_link",
            vec![],
        )
        .await
    }

    async fn record_join(
        &self,
        server_id: ServerId,
        user_id: UserId,
        at: DateTime<Utc>,
    ) -> RepoResult<()> {
        self.execute(
            "DELETE FROM workspace_public_link_join WHERE server_id = $1 AND joined_at < $2",
            vec![uuid(server_id), utc(at - Duration::days(JOIN_RECORD_DAYS))],
        )
        .await?;
        self.execute(
            "INSERT INTO workspace_public_link_join (server_id, user_id, joined_at) \
             VALUES ($1, $2, $3)",
            vec![uuid(server_id), uuid(user_id), utc(at)],
        )
        .await?;
        Ok(())
    }

    async fn count_joins_since(
        &self,
        server_id: ServerId,
        since: DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.count(
            "SELECT count(*) AS count FROM workspace_public_link_join \
             WHERE server_id = $1 AND joined_at >= $2",
            vec![uuid(server_id), utc(since)],
        )
        .await
    }

    async fn create_request(&self, request: NewJoinRequest) -> RepoResult<JoinRequest> {
        self.execute(
            "UPDATE workspace_join_request SET status = 'expired' \
             WHERE server_id = $1 AND user_id = $2 AND status = 'open' AND expires_at <= $3",
            vec![
                uuid(request.server_id),
                uuid(request.user_id),
                utc(request.created_at),
            ],
        )
        .await?;
        // `DO NOTHING` rather than a unique violation, which would abort the caller's
        // transaction.
        let sql = format!(
            "INSERT INTO workspace_join_request \
                 (id, server_id, user_id, message, status, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, 'open', $5, $6) \
             ON CONFLICT (server_id, user_id) WHERE status = 'open' DO NOTHING \
             RETURNING {REQUEST_COLUMNS}"
        );
        self.requests(
            &sql,
            vec![
                uuid(JoinRequestId::new()),
                uuid(request.server_id),
                uuid(request.user_id),
                request.message.into(),
                utc(request.created_at),
                utc(request.expires_at),
            ],
        )
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            RepoError::with_message(RepoErrorKind::Conflict, "an open join request exists")
        })
    }

    async fn request(&self, id: JoinRequestId) -> RepoResult<Option<JoinRequest>> {
        Ok(self
            .requests(
                &format!("SELECT {REQUEST_COLUMNS} FROM workspace_join_request WHERE id = $1"),
                vec![uuid(id)],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn open_request(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<JoinRequest>> {
        Ok(self
            .requests(
                &format!(
                    "SELECT {REQUEST_COLUMNS} FROM workspace_join_request \
                     WHERE server_id = $1 AND user_id = $2 AND status = 'open' \
                     AND expires_at > $3"
                ),
                vec![uuid(server_id), uuid(user_id), utc(now)],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn last_declined_at(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<DateTime<Utc>>> {
        let row: Option<AtRow> = self
            .one(
                "SELECT max(decided_at) AS at FROM workspace_join_request \
                 WHERE server_id = $1 AND user_id = $2 AND status = 'declined'",
                vec![uuid(server_id), uuid(user_id)],
            )
            .await?;
        Ok(row.and_then(|r| r.at).map(Into::into))
    }

    async fn count_open_for_user(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<u64> {
        self.count(
            "SELECT count(*) AS count FROM workspace_join_request \
             WHERE user_id = $1 AND status = 'open' AND expires_at > $2",
            vec![uuid(user_id), utc(now)],
        )
        .await
    }

    async fn count_open_for_server(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.count(
            "SELECT count(*) AS count FROM workspace_join_request \
             WHERE server_id = $1 AND status = 'open' AND expires_at > $2",
            vec![uuid(server_id), utc(now)],
        )
        .await
    }

    async fn list_open_requests(
        &self,
        params: ListJoinRequestsParams,
    ) -> RepoResult<Page<JoinRequest, JoinRequestCursor>> {
        let limit = Ord::max(params.limit, 1);
        let mut values = vec![uuid(params.server_id), utc(params.now)];
        // Newest first (DESC endpoint): `After` = older than the cursor, `Before` = newer.
        let has_cursor = params.cursor.is_some();
        let condition = match params.cursor {
            Some(cursor) => {
                values.push(utc(cursor.created_at));
                values.push(uuid(cursor.request_id));
                match params.dir {
                    PageDir::After => "AND (created_at, id) < ($3, $4)",
                    PageDir::Before => "AND (created_at, id) > ($3, $4)",
                }
            }
            None => "",
        };
        // `Before` pages are fetched nearest-first (ascending) and flipped back.
        let fetch_reversed = has_cursor && params.dir == PageDir::Before;
        let order = if fetch_reversed {
            "created_at ASC, id ASC"
        } else {
            "created_at DESC, id DESC"
        };
        let sql = format!(
            "SELECT {REQUEST_COLUMNS} FROM workspace_join_request \
             WHERE server_id = $1 AND status = 'open' AND expires_at > $2 {condition} \
             ORDER BY {order} LIMIT {}",
            u64::from(limit) + 1
        );
        let mut items = self.requests(&sql, values).await?;

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }
        if fetch_reversed {
            items.reverse();
        }

        let to_cursor = |r: &JoinRequest| JoinRequestCursor {
            created_at: r.created_at,
            request_id: r.id,
        };
        // Items are newest first: `first` is the max, `last` the min.
        let max = items.first().map(to_cursor);
        let min = items.last().map(to_cursor);
        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match params.dir {
            PageDir::After => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };
        Ok(Page::new(items, next, prev))
    }

    async fn list_open_for_user(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<JoinRequest>> {
        self.requests(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM workspace_join_request \
                 WHERE user_id = $1 AND status = 'open' AND expires_at > $2 \
                 ORDER BY created_at DESC, id DESC"
            ),
            vec![uuid(user_id), utc(now)],
        )
        .await
    }

    async fn close_request(
        &self,
        id: JoinRequestId,
        status: JoinRequestStatus,
        decided_by: Option<UserId>,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<JoinRequest>> {
        Ok(self
            .requests(
                &format!(
                    "UPDATE workspace_join_request \
                     SET status = $2, decided_by_user_id = $3, decided_at = $4 \
                     WHERE id = $1 AND status = 'open' AND expires_at > $4 \
                     RETURNING {REQUEST_COLUMNS}"
                ),
                vec![
                    uuid(id),
                    status.as_str().into(),
                    decided_by.map(Uuid::from).into(),
                    utc(now),
                ],
            )
            .await?
            .into_iter()
            .next())
    }

    async fn cancel_open_requests(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<JoinRequest>> {
        self.requests(
            &format!(
                "UPDATE workspace_join_request SET status = 'cancelled', decided_at = $3 \
                 WHERE server_id = $1 AND user_id = $2 AND status = 'open' AND expires_at > $3 \
                 RETURNING {REQUEST_COLUMNS}"
            ),
            vec![uuid(server_id), uuid(user_id), utc(now)],
        )
        .await
    }
}
