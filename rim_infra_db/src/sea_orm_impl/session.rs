use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::session;
use sea_orm::prelude::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

use crate::error::map_db_err;
use rim_core::auth::model::{NewSession, Session, SessionId};
use rim_core::auth::repository::SessionRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;

pub struct SeaOrmSessionRepository {
    db: DbConn,
}

impl SeaOrmSessionRepository {
    pub fn new(db: DbConn) -> Self {
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
        let Some(model) = session::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active_model: session::ActiveModel = model.into();
        active_model.last_seen_at = Set(Some(now.into()));
        active_model.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn revoke_session(&self, id: SessionId) -> RepoResult<()> {
        let now: DateTime<Utc> = Utc::now();
        let Some(model) = session::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active_model: session::ActiveModel = model.into();
        active_model.revoked_at = Set(Some(now.into()));
        active_model.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64> {
        let now: DateTime<Utc> = Utc::now();
        let res = session::Entity::update_many()
            .filter(session::Column::UserId.eq(sea_orm::Value::Uuid(Some(user_id.into()))))
            .col_expr(session::Column::RevokedAt, Expr::value(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}
