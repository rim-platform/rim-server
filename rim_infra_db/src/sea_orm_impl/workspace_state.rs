use async_trait::async_trait;
use entity::workspace_state;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_state::model::{
    NewWorkspaceState, UpdateWorkspaceState, WorkspaceState, WorkspaceStateId, WorkspaceStateScope,
};
use rim_core::workspace_state::repository::WorkspaceStateRepository;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmWorkspaceStateRepository {
    db: DbHandle,
}

impl SeaOrmWorkspaceStateRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn scope_filter(scope: WorkspaceStateScope, user_id: Option<UserId>) -> Condition {
        let mut cond = Condition::all().add(workspace_state::Column::Scope.eq(scope.as_str()));
        match scope {
            WorkspaceStateScope::User => {
                if let Some(uid) = user_id {
                    cond = cond.add(workspace_state::Column::UserId.eq::<Uuid>(uid.into()));
                }
            }
            WorkspaceStateScope::Server => {
                cond = cond.add(workspace_state::Column::UserId.is_null());
            }
        }
        cond
    }

    fn not_expired_filter() -> Condition {
        Condition::any()
            .add(workspace_state::Column::ExpiresAt.is_null())
            .add(workspace_state::Column::ExpiresAt.gt(chrono::Utc::now()))
    }
}

#[async_trait]
impl WorkspaceStateRepository for SeaOrmWorkspaceStateRepository {
    async fn get_by_id(&self, id: WorkspaceStateId) -> RepoResult<Option<WorkspaceState>> {
        let model = workspace_state::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
    ) -> RepoResult<Option<WorkspaceState>> {
        let model = workspace_state::Entity::find()
            .filter(workspace_state::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(workspace_state::Column::Key.eq(key))
            .filter(Self::scope_filter(scope, user_id))
            .filter(Self::not_expired_filter())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_by_prefix(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        prefix: &str,
        limit: u64,
    ) -> RepoResult<Vec<WorkspaceState>> {
        let models = workspace_state::Entity::find()
            .filter(workspace_state::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(workspace_state::Column::Key.starts_with(prefix))
            .filter(Self::scope_filter(scope, user_id))
            .filter(Self::not_expired_filter())
            .order_by_asc(workspace_state::Column::Key)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn count(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
    ) -> RepoResult<u64> {
        let count = workspace_state::Entity::find()
            .filter(workspace_state::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(Self::scope_filter(scope, user_id))
            .filter(Self::not_expired_filter())
            .count(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(count)
    }

    async fn upsert(&self, new: NewWorkspaceState) -> RepoResult<WorkspaceState> {
        let now = chrono::Utc::now();
        let id = WorkspaceStateId::new();
        let scope_str = new.scope.as_str().to_string();

        // Use raw SQL for PostgreSQL ON CONFLICT DO UPDATE
        let user_id_val: Option<Uuid> = new.user_id.map(Into::into);
        let server_id_val: Uuid = new.server_id.into();
        let created_by_val: Uuid = new.created_by.into();

        let result = sea_orm::FromQueryResult::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            r#"
            INSERT INTO workspace_state (id, server_id, key, value, scope, user_id, created_by, updated_by, version, expires_at, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $7, 1, $8, $9, $9)
            ON CONFLICT (server_id, scope, user_id, key)
            DO UPDATE SET value = $4, updated_by = $7, version = workspace_state.version + 1, expires_at = $8, updated_at = $9
            RETURNING *
            "#,
            vec![
                sea_orm::Value::Uuid(Some(id.into())),
                sea_orm::Value::Uuid(Some(server_id_val)),
                sea_orm::Value::String(Some(new.key)),
                sea_orm::Value::Json(Some(Box::new(new.value))),
                sea_orm::Value::String(Some(scope_str)),
                match user_id_val {
                    Some(uid) => sea_orm::Value::Uuid(Some(uid)),
                    None => sea_orm::Value::Uuid(None),
                },
                sea_orm::Value::Uuid(Some(created_by_val)),
                match new.expires_at {
                    Some(ts) => sea_orm::Value::ChronoDateTimeUtc(Some(ts)),
                    None => sea_orm::Value::ChronoDateTimeUtc(None),
                },
                sea_orm::Value::ChronoDateTimeUtc(Some(now)),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;

        let model: workspace_state::Model =
            result.ok_or_else(|| RepoError::new(RepoErrorKind::Internal))?;
        Ok(model.into())
    }

    async fn update(
        &self,
        id: WorkspaceStateId,
        update: UpdateWorkspaceState,
    ) -> RepoResult<WorkspaceState> {
        let model = workspace_state::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        if let Some(expected) = update.expected_version
            && model.version != expected
        {
            return Err(RepoError::with_message(
                RepoErrorKind::PreconditionFailed,
                format!(
                    "version mismatch: expected {expected}, actual {}",
                    model.version
                ),
            ));
        }

        let next_version = model.version + 1;
        let mut active: workspace_state::ActiveModel = model.into();
        active.value = Set(update.value);
        active.updated_by = Set(update.updated_by.into());
        active.version = Set(next_version);
        active.expires_at = Set(update.expires_at.map(|dt| dt.into()));
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: WorkspaceStateId) -> RepoResult<()> {
        let model = workspace_state::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let active: workspace_state::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn delete_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
    ) -> RepoResult<bool> {
        let result = workspace_state::Entity::delete_many()
            .filter(workspace_state::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(workspace_state::Column::Key.eq(key))
            .filter(Self::scope_filter(scope, user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn delete_expired(&self) -> RepoResult<u64> {
        let result = workspace_state::Entity::delete_many()
            .filter(workspace_state::Column::ExpiresAt.is_not_null())
            .filter(workspace_state::Column::ExpiresAt.lte(chrono::Utc::now()))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected)
    }

    async fn increment_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
        delta: i64,
        actor_id: UserId,
    ) -> RepoResult<WorkspaceState> {
        let now = chrono::Utc::now();
        let id = WorkspaceStateId::new();
        let scope_str = scope.as_str().to_string();
        let user_id_val: Option<Uuid> = user_id.map(Into::into);
        let server_id_val: Uuid = server_id.into();
        let actor_id_val: Uuid = actor_id.into();

        let result = sea_orm::FromQueryResult::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            r#"
            INSERT INTO workspace_state (id, server_id, key, value, scope, user_id, created_by, updated_by, version, expires_at, created_at, updated_at)
            VALUES ($1, $2, $3, to_jsonb($4::bigint), $5, $6, $7, $7, 1, NULL, $8, $8)
            ON CONFLICT (server_id, scope, user_id, key)
            DO UPDATE SET value = to_jsonb((workspace_state.value::text::bigint + $4::bigint)), updated_by = $7, version = workspace_state.version + 1, updated_at = $8
            RETURNING *
            "#,
            vec![
                sea_orm::Value::Uuid(Some(id.into())),
                sea_orm::Value::Uuid(Some(server_id_val)),
                sea_orm::Value::String(Some(key.to_string())),
                sea_orm::Value::BigInt(Some(delta)),
                sea_orm::Value::String(Some(scope_str)),
                match user_id_val {
                    Some(uid) => sea_orm::Value::Uuid(Some(uid)),
                    None => sea_orm::Value::Uuid(None),
                },
                sea_orm::Value::Uuid(Some(actor_id_val)),
                sea_orm::Value::ChronoDateTimeUtc(Some(now)),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;

        let model: workspace_state::Model =
            result.ok_or_else(|| RepoError::new(RepoErrorKind::Internal))?;
        Ok(model.into())
    }
}
