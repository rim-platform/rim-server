use async_trait::async_trait;
use rim_core::auth::model::InstanceInviteId;
use rim_core::default_workspace::model::DefaultWorkspaceRecord;
use rim_core::default_workspace::repository::DefaultWorkspaceRepository;
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value};

use crate::DbHandle;
use crate::error::map_db_err;

/// The default workspace record on the one row of `instance_default_workspace`.
pub struct SeaOrmDefaultWorkspaceRepository {
    db: DbHandle,
}

impl SeaOrmDefaultWorkspaceRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        Ok(self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .map_err(map_db_err)?
            .rows_affected())
    }
}

#[derive(FromQueryResult)]
struct RecordRow {
    workspace_id: Option<Uuid>,
    claim_invite_id: Option<Uuid>,
    claimed_by_user_id: Option<Uuid>,
    claimed_at: Option<DateTimeWithTimeZone>,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[async_trait]
impl DefaultWorkspaceRepository for SeaOrmDefaultWorkspaceRepository {
    async fn record(&self) -> RepoResult<DefaultWorkspaceRecord> {
        let row = RecordRow::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "SELECT workspace_id, claim_invite_id, claimed_by_user_id, claimed_at \
             FROM instance_default_workspace WHERE id = 1",
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.map_or_else(DefaultWorkspaceRecord::default, |row| {
            DefaultWorkspaceRecord {
                workspace_id: row.workspace_id.map(Into::into),
                claim_invite_id: row.claim_invite_id.map(Into::into),
                claimed_by: row.claimed_by_user_id.map(Into::into),
                claimed_at: row.claimed_at.map(Into::into),
            }
        }))
    }

    async fn set_claim_invite(&self, invite: InstanceInviteId) -> RepoResult<()> {
        self.execute(
            "INSERT INTO instance_default_workspace (id, claim_invite_id) VALUES (1, $1) \
             ON CONFLICT (id) DO UPDATE SET claim_invite_id = EXCLUDED.claim_invite_id, \
             updated_at = now()",
            vec![Uuid::from(invite).into()],
        )
        .await?;
        Ok(())
    }

    async fn take_claim(&self, invite: InstanceInviteId) -> RepoResult<bool> {
        let taken = self
            .execute(
                "UPDATE instance_default_workspace SET claim_invite_id = NULL, updated_at = now() \
                 WHERE id = 1 AND claim_invite_id = $1 AND workspace_id IS NULL",
                vec![Uuid::from(invite).into()],
            )
            .await?;
        Ok(taken == 1)
    }

    async fn record_claim(&self, workspace: ServerId, owner: UserId) -> RepoResult<()> {
        self.execute(
            "INSERT INTO instance_default_workspace \
                 (id, workspace_id, claimed_by_user_id, claimed_at) VALUES (1, $1, $2, now()) \
             ON CONFLICT (id) DO UPDATE SET workspace_id = EXCLUDED.workspace_id, \
                 claim_invite_id = NULL, claimed_by_user_id = EXCLUDED.claimed_by_user_id, \
                 claimed_at = EXCLUDED.claimed_at, updated_at = now()",
            vec![Uuid::from(workspace).into(), Uuid::from(owner).into()],
        )
        .await?;
        Ok(())
    }

    async fn other_local_accounts(&self, except: UserId) -> RepoResult<Vec<UserId>> {
        let rows = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id FROM "user" WHERE kind = 'local' AND deleted_at IS NULL AND id <> $1
               ORDER BY created_at, id"#,
            vec![Uuid::from(except).into()],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows.into_iter().map(|row| row.id.into()).collect())
    }
}
