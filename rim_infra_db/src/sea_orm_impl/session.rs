use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::session;
use sea_orm::prelude::Expr;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set, Statement,
};

use crate::DbHandle;

use crate::error::map_db_err;
use rim_core::auth::model::{NewSession, Session, SessionId, SessionKeep};
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
        self.db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO user_activity (user_id, last_authenticated_at)
               SELECT u.id, $2 FROM session s JOIN "user" u ON u.id = s.user_id
               WHERE s.id = $1 AND s.revoked_at IS NULL AND s.expires_at > $2
                 AND u.kind = 'local' AND u.deleted_at IS NULL
                 AND NOT EXISTS (SELECT 1 FROM user_activity a WHERE a.user_id = u.id
                     AND a.last_authenticated_at >= $2 - interval '5 minutes')
               ON CONFLICT (user_id) DO UPDATE
               SET last_authenticated_at = EXCLUDED.last_authenticated_at
               WHERE user_activity.last_authenticated_at < EXCLUDED.last_authenticated_at - interval '5 minutes'"#,
            vec![sea_orm::Value::Uuid(Some(id.into())), now.into()],
        )).await.map_err(map_db_err)?;
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

    async fn delete_ended(&self, now: DateTime<Utc>) -> RepoResult<u64> {
        let res = session::Entity::delete_many()
            .filter(
                Condition::any()
                    .add(session::Column::RevokedAt.is_not_null())
                    .add(session::Column::ExpiresAt.lte(now)),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn set_keep(&self, id: SessionId, keep: SessionKeep) -> RepoResult<()> {
        let (kept, secret_hash, keep_key_sealed) = match keep {
            SessionKeep::Kept { keep_key_sealed } => (true, None, keep_key_sealed),
            SessionKeep::Unkept { secret_hash } => (false, Some(secret_hash), None),
        };
        session::Entity::update_many()
            .col_expr(session::Column::Kept, Expr::value(kept))
            .col_expr(session::Column::SecretHash, Expr::value(secret_hash))
            .col_expr(session::Column::KeepKeySealed, Expr::value(keep_key_sealed))
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn set_reload_key(&self, id: SessionId, reload_key_sealed: Vec<u8>) -> RepoResult<bool> {
        let res = session::Entity::update_many()
            .col_expr(
                session::Column::ReloadKeySealed,
                Expr::value(Some(reload_key_sealed)),
            )
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .filter(session::Column::Kept.eq(false))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected > 0)
    }

    async fn take_reload_key(
        &self,
        id: SessionId,
        next_sealed: Vec<u8>,
        now: DateTime<Utc>,
        closed_since: DateTime<Utc>,
    ) -> RepoResult<Option<Vec<u8>>> {
        // The row lock of the inner select makes two concurrent calls take the key once.
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE session s
                   SET reload_key_sealed = $2, ws_closed_at = NULL
                   FROM (SELECT id, reload_key_sealed FROM session WHERE id = $1 FOR UPDATE) old
                   WHERE s.id = old.id
                     AND s.kept = false
                     AND s.revoked_at IS NULL
                     AND s.expires_at > $3
                     AND s.ws_closed_at > $4
                     AND s.ws_closed_at <= $3
                     AND old.reload_key_sealed IS NOT NULL
                   RETURNING old.reload_key_sealed"#,
                vec![
                    sea_orm::Value::Uuid(Some(id.into())),
                    next_sealed.into(),
                    now.into(),
                    closed_since.into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        row.map(|row| row.try_get::<Vec<u8>>("", "reload_key_sealed"))
            .transpose()
            .map_err(map_db_err)
    }

    async fn socket_opened(&self, id: SessionId) -> RepoResult<()> {
        session::Entity::update_many()
            .col_expr(
                session::Column::WsClosedAt,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .filter(session::Column::Kept.eq(false))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn socket_closed(&self, id: SessionId, at: DateTime<Utc>) -> RepoResult<()> {
        session::Entity::update_many()
            .col_expr(session::Column::WsClosedAt, Expr::value(Some(at)))
            .filter(session::Column::Id.eq(sea_orm::Value::Uuid(Some(id.into()))))
            .filter(session::Column::Kept.eq(false))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}
