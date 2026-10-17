use std::str::FromStr;

use async_trait::async_trait;
use rim_core::dm_policy::model::{
    MemberDm, OutsideDm, OutsideFloor, SharedWorkspace, UserWorkspaceDm, WorkspaceDmPolicy,
};
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
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

fn invalid() -> RepoError {
    RepoError::new(RepoErrorKind::InvalidData)
}

#[derive(FromQueryResult)]
struct PolicyRow {
    id: Uuid,
    dm_outside_floor: String,
    dm_member_default: String,
}

impl TryFrom<PolicyRow> for WorkspaceDmPolicy {
    type Error = RepoError;

    fn try_from(row: PolicyRow) -> RepoResult<Self> {
        Ok(Self {
            server_id: row.id.into(),
            outside_floor: OutsideFloor::parse(&row.dm_outside_floor).ok_or_else(invalid)?,
            member_default: MemberDm::parse(&row.dm_member_default).ok_or_else(invalid)?,
        })
    }
}

#[derive(FromQueryResult)]
struct OutsideRow {
    outside: String,
}

#[derive(FromQueryResult)]
struct WorkspaceRow {
    server_id: Uuid,
    name: String,
    dm_outside_floor: String,
    dm_member_default: String,
    choice: Option<String>,
}

impl TryFrom<WorkspaceRow> for UserWorkspaceDm {
    type Error = RepoError;

    fn try_from(row: WorkspaceRow) -> RepoResult<Self> {
        Ok(Self {
            server_id: row.server_id.into(),
            name: row.name,
            outside_floor: OutsideFloor::parse(&row.dm_outside_floor).ok_or_else(invalid)?,
            member_default: MemberDm::parse(&row.dm_member_default).ok_or_else(invalid)?,
            choice: row
                .choice
                .map(|c| MemberDm::parse(&c).ok_or_else(invalid))
                .transpose()?,
        })
    }
}

#[derive(FromQueryResult)]
struct SharedRow {
    server_id: Uuid,
    name: String,
    dm_outside_floor: String,
    dm_member_default: String,
    choice: Option<String>,
    sender_role: String,
}

/// The columns of a user's workspaces with their DM rules and the user's choice; `a` is the
/// user's membership.
const WORKSPACE_COLUMNS: &str = "s.id AS server_id, s.name, s.dm_outside_floor, \
     s.dm_member_default, c.members AS choice";

const WORKSPACE_JOINS: &str = "FROM server_member a \
     JOIN server s ON s.id = a.server_id AND s.kind = 'workspace' \
     LEFT JOIN user_workspace_dm_setting c ON c.user_id = a.user_id AND c.server_id = s.id";

/// Workspace DM rules on `server`, and each user's choices on `user_dm_setting` and
/// `user_workspace_dm_setting`, in raw SQL.
pub struct SeaOrmDmPolicyRepository {
    db: DbHandle,
}

impl SeaOrmDmPolicyRepository {
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

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<()> {
        self.db
            .execute_raw(statement(sql, values))
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}

#[async_trait]
impl DmPolicyRepository for SeaOrmDmPolicyRepository {
    async fn workspace_policy(&self, server_id: ServerId) -> RepoResult<Option<WorkspaceDmPolicy>> {
        self.one::<PolicyRow>(
            "SELECT id, dm_outside_floor, dm_member_default FROM server \
             WHERE id = $1 AND kind = 'workspace'",
            vec![uuid(server_id)],
        )
        .await?
        .map(TryInto::try_into)
        .transpose()
    }

    async fn set_workspace_policy(
        &self,
        server_id: ServerId,
        outside_floor: OutsideFloor,
        member_default: MemberDm,
    ) -> RepoResult<Option<WorkspaceDmPolicy>> {
        self.one::<PolicyRow>(
            "UPDATE server SET dm_outside_floor = $2, dm_member_default = $3, updated_at = now() \
             WHERE id = $1 AND kind = 'workspace' \
             RETURNING id, dm_outside_floor, dm_member_default",
            vec![
                uuid(server_id),
                outside_floor.as_str().into(),
                member_default.as_str().into(),
            ],
        )
        .await?
        .map(TryInto::try_into)
        .transpose()
    }

    async fn outside(&self, user_id: UserId) -> RepoResult<OutsideDm> {
        match self
            .one::<OutsideRow>(
                "SELECT outside FROM user_dm_setting WHERE user_id = $1",
                vec![uuid(user_id)],
            )
            .await?
        {
            Some(row) => OutsideDm::parse(&row.outside).ok_or_else(invalid),
            None => Ok(OutsideDm::default()),
        }
    }

    async fn set_outside(&self, user_id: UserId, outside: OutsideDm) -> RepoResult<()> {
        self.execute(
            "INSERT INTO user_dm_setting (user_id, outside) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET outside = EXCLUDED.outside, updated_at = now()",
            vec![uuid(user_id), outside.as_str().into()],
        )
        .await
    }

    async fn set_member_choice(
        &self,
        user_id: UserId,
        server_id: ServerId,
        members: MemberDm,
    ) -> RepoResult<()> {
        self.execute(
            "INSERT INTO user_workspace_dm_setting (user_id, server_id, members) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, server_id) \
             DO UPDATE SET members = EXCLUDED.members, updated_at = now()",
            vec![uuid(user_id), uuid(server_id), members.as_str().into()],
        )
        .await
    }

    async fn user_workspaces(&self, user_id: UserId) -> RepoResult<Vec<UserWorkspaceDm>> {
        self.all::<WorkspaceRow>(
            &format!(
                "SELECT {WORKSPACE_COLUMNS} {WORKSPACE_JOINS} WHERE a.user_id = $1 \
                 ORDER BY s.created_at, s.id"
            ),
            vec![uuid(user_id)],
        )
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
    }

    async fn shared_workspaces(
        &self,
        user_id: UserId,
        principal_id: &str,
    ) -> RepoResult<Vec<SharedWorkspace>> {
        self.all::<SharedRow>(
            &format!(
                r#"SELECT {WORKSPACE_COLUMNS}, b.member_role::text AS sender_role {WORKSPACE_JOINS}
                   JOIN server_member b ON b.server_id = a.server_id AND b.user_id <> a.user_id
                   JOIN "user" u ON u.id = b.user_id
                   WHERE a.user_id = $1 AND u.principal_id = $2
                   ORDER BY s.created_at, s.id"#
            ),
            vec![uuid(user_id), principal_id.into()],
        )
        .await?
        .into_iter()
        .map(|row| {
            let sender_role =
                ServerMemberRole::from_str(&row.sender_role).map_err(|_| invalid())?;
            Ok(SharedWorkspace {
                workspace: WorkspaceRow {
                    server_id: row.server_id,
                    name: row.name,
                    dm_outside_floor: row.dm_outside_floor,
                    dm_member_default: row.dm_member_default,
                    choice: row.choice,
                }
                .try_into()?,
                sender_role,
            })
        })
        .collect()
    }
}
