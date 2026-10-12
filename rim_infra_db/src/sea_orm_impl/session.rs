use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::session;
use sea_orm::prelude::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;

use crate::error::map_db_err;
use rim_core::auth::model::{NewSession, Session, SessionId};
use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;

pub struct SeaOrmSessionRepository {
    db: DbHandle,
}

impl SeaOrmSessionRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SessionRepository for SeaOrmSessionRepository {
    async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>> {
        let model = session::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>> {
        let model = session::Entity::find()
            .filter(session::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_many(&self, ids: &[SessionId]) -> RepoResult<Vec<Session>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let models = session::Entity::find()
            .filter(
                session::Column::Id.is_in(ids.iter().map(|id| sea_orm::prelude::Uuid::from(*id))),
            )
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create_session(&self, s: NewSession) -> RepoResult<Session> {
        let active_model = session::ActiveModel {
            id: Set(SessionId::new().into()),
            user_id: Set(s.user_id.into()),
            token_hash: Set(s.token_hash),
            expires_at: Set(s.expires_at.into()),
            // Newly created sessions are active.
            last_seen_at: Set(None),
            revoked_at: Set(None),
            user_agent: Set(s.user_agent),
            ip: Set(s.ip),
            ..Default::default()
        };

        let res = session::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(res.into())
    }

    async fn touch_last_seen_at(&self, id: SessionId) -> RepoResult<()> {
        let now: DateTime<Utc> = Utc::now();
        session::Entity::update_many()
            .col_expr(session::Column::LastSeenAt, Expr::value(Some(now)))
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn revoke_session(&self, id: SessionId) -> RepoResult<()> {
        let now: DateTime<Utc> = Utc::now();
        session::Entity::update_many()
            .col_expr(session::Column::RevokedAt, Expr::value(Some(now)))
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn bind_device(&self, id: SessionId, device_key_id: DeviceKeyId) -> RepoResult<()> {
        session::Entity::update_many()
            .col_expr(
                session::Column::DeviceKeyId,
                Expr::value(Some(sea_orm::prelude::Uuid::from(device_key_id))),
            )
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn revoke_all_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64> {
        let now: DateTime<Utc> = Utc::now();
        let res = session::Entity::update_many()
            .filter(session::Column::DeviceKeyId.eq(sea_orm::prelude::Uuid::from(device_key_id)))
            .filter(session::Column::RevokedAt.is_null())
            .col_expr(session::Column::RevokedAt, Expr::value(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64> {
        let now: DateTime<Utc> = Utc::now();
        let res = session::Entity::update_many()
            .filter(session::Column::UserId.eq(sea_orm::Value::Uuid(Some(user_id.into()))))
            .filter(session::Column::RevokedAt.is_null())
            .col_expr(session::Column::RevokedAt, Expr::value(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn revoke_all_for_user_except(
        &self,
        user_id: UserId,
        keep: SessionId,
    ) -> RepoResult<u64> {
        let now: DateTime<Utc> = Utc::now();
        let res = session::Entity::update_many()
            .filter(session::Column::UserId.eq(sea_orm::Value::Uuid(Some(user_id.into()))))
            .filter(session::Column::Id.ne(sea_orm::Value::Uuid(Some(keep.into()))))
            .filter(session::Column::RevokedAt.is_null())
            .col_expr(session::Column::RevokedAt, Expr::value(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}
