use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::instance_invite;
use sea_orm::Value;
use sea_orm::prelude::{Expr, Uuid};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, ExprTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::auth::model::{InstanceInvite, InstanceInviteId, NewInstanceInvite};
use rim_core::auth::repository::InstanceInviteRepository;
use rim_core::repository::error::RepoResult;

/// SeaORM-backed implementation of [`InstanceInviteRepository`].
pub struct SeaOrmInstanceInviteRepository {
    db: DbHandle,
}

impl SeaOrmInstanceInviteRepository {
    /// Construct a new repository using the given [`DbHandle`].
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InstanceInviteRepository for SeaOrmInstanceInviteRepository {
    /// Create a new instance invite row.
    ///
    /// # Errors
    /// Returns [`RepoErrorKind::Conflict`] if `token_hash` already exists.
    ///
    /// # Examples
    /// ```ignore
    /// let invite = repo.create(NewInstanceInvite {
    ///     created_by_user_id: actor.user_id,
    ///     token_hash: hash_token(&raw),
    ///     expires_at: Utc::now() + Duration::days(7),
    ///     max_uses: 1,
    /// }).await?;
    /// ```
    async fn create(&self, invite: NewInstanceInvite) -> RepoResult<InstanceInvite> {
        let active_model = instance_invite::ActiveModel {
            id: Set(InstanceInviteId::new().into()),
            created_by_user_id: Set(invite.created_by_user_id.into()),
            token_hash: Set(invite.token_hash),
            expires_at: Set(invite.expires_at.into()),
            max_uses: Set(invite.max_uses),
            ..Default::default()
        };

        let res = instance_invite::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    /// Retrieve an instance invite by its primary key.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let maybe = repo.get_by_id(invite_id).await?;
    /// ```
    async fn get_by_id(&self, id: InstanceInviteId) -> RepoResult<Option<InstanceInvite>> {
        let model = instance_invite::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    /// Retrieve an instance invite by its SHA-256 hash.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let maybe = repo.get_by_token_hash(&hash_token(&raw)).await?;
    /// ```
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<InstanceInvite>> {
        let model = instance_invite::Entity::find()
            .filter(instance_invite::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    /// Atomically consume one use slot.
    ///
    /// Increments `use_count` only when all of the following hold (M4 hardening):
    /// - `use_count < max_uses` (capacity available)
    /// - `revoked_at IS NULL` (not revoked — guards against concurrent admin revoke)
    /// - `expires_at > now()` (not expired)
    ///
    /// Returns `true` on success, `false` when any condition fails or the row
    /// is missing. All conditions are evaluated inside a single UPDATE statement,
    /// so there is no TOCTOU window between the pre-check and the increment.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let consumed = repo.try_consume(invite.id).await?;
    /// if !consumed {
    ///     return Err(ServiceError::InvalidToken);
    /// }
    /// ```
    async fn try_consume(&self, id: InstanceInviteId) -> RepoResult<bool> {
        let now = Utc::now();
        let res = instance_invite::Entity::update_many()
            .filter(instance_invite::Column::Id.eq::<Uuid>(id.into()))
            // capacity check
            .filter(
                sea_orm::Condition::all().add(
                    Expr::col(instance_invite::Column::UseCount)
                        .lt(Expr::col(instance_invite::Column::MaxUses)),
                ),
            )
            // M4: must not be revoked (guards concurrent admin revoke)
            .filter(instance_invite::Column::RevokedAt.is_null())
            // M4: must not be expired
            .filter(instance_invite::Column::ExpiresAt.gt(Value::ChronoDateTimeUtc(Some(now))))
            .col_expr(
                instance_invite::Column::UseCount,
                Expr::col(instance_invite::Column::UseCount).add(1),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(res.rows_affected == 1)
    }

    /// Revoke an instance invite by setting `revoked_at = now()`.
    ///
    /// No-op if the row does not exist.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// repo.revoke(invite.id).await?;
    /// ```
    async fn revoke(&self, id: InstanceInviteId) -> RepoResult<()> {
        let now: DateTime<Utc> = Utc::now();
        let Some(model) = instance_invite::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active: instance_invite::ActiveModel = model.into();
        active.revoked_at = Set(Some(now.into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    /// List all non-revoked instance invites.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let active = repo.list_active().await?;
    /// ```
    async fn list_active(&self) -> RepoResult<Vec<InstanceInvite>> {
        let models = instance_invite::Entity::find()
            .filter(instance_invite::Column::RevokedAt.is_null())
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }
}
