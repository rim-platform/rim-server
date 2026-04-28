use async_trait::async_trait;
use chrono::Utc;
use entity::bootstrap_token;
use sea_orm::Value;
use sea_orm::prelude::{Expr, Uuid};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::auth::model::{BootstrapToken, BootstrapTokenId, NewBootstrapToken};
use rim_core::auth::repository::BootstrapTokenRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;

/// SeaORM-backed implementation of [`BootstrapTokenRepository`].
pub struct SeaOrmBootstrapTokenRepository {
    db: DbHandle,
}

impl SeaOrmBootstrapTokenRepository {
    /// Construct a new repository using the given [`DbHandle`].
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl BootstrapTokenRepository for SeaOrmBootstrapTokenRepository {
    /// Create a new bootstrap token row.
    ///
    /// # Errors
    /// Returns [`RepoErrorKind::Conflict`] if `token_hash` already exists in the table.
    ///
    /// # Examples
    /// ```ignore
    /// let token = repo.create(NewBootstrapToken {
    ///     email: "admin@example.com".into(),
    ///     token_hash: hash_token(&raw_token),
    ///     expires_at: Utc::now() + Duration::minutes(15),
    /// }).await?;
    /// ```
    async fn create(&self, token: NewBootstrapToken) -> RepoResult<BootstrapToken> {
        let active_model = bootstrap_token::ActiveModel {
            id: Set(BootstrapTokenId::new().into()),
            email: Set(token.email),
            token_hash: Set(token.token_hash),
            expires_at: Set(token.expires_at.into()),
            ..Default::default()
        };

        let res = bootstrap_token::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    /// Retrieve a bootstrap token by its primary key.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let maybe = repo.get_by_id(token_id).await?;
    /// ```
    async fn get_by_id(&self, id: BootstrapTokenId) -> RepoResult<Option<BootstrapToken>> {
        let model = bootstrap_token::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    /// Retrieve a bootstrap token by its SHA-256 hash.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let maybe = repo.get_by_token_hash(&hash_token(&raw)).await?;
    /// ```
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<BootstrapToken>> {
        let model = bootstrap_token::Entity::find()
            .filter(bootstrap_token::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    /// Atomically mark a bootstrap token as used, recording the consuming user.
    ///
    /// Executes a single `UPDATE … WHERE id = $1 AND used_at IS NULL AND
    /// revoked_at IS NULL` statement, eliminating the check-then-update race
    /// present in a two-step read + write (C1 hardening).
    ///
    /// Returns `true` when the row was updated (the token was still available),
    /// `false` when another concurrent consumer already claimed it, the row
    /// does not exist, or the token is revoked.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let claimed = repo.try_mark_used(token.id, user.id).await?;
    /// if !claimed {
    ///     return Err(ServiceError::InvalidToken);
    /// }
    /// ```
    async fn try_mark_used(
        &self,
        id: BootstrapTokenId,
        consumed_by_user_id: UserId,
    ) -> RepoResult<bool> {
        let now = Utc::now();
        let user_uuid: Uuid = consumed_by_user_id.into();
        let res = bootstrap_token::Entity::update_many()
            .filter(bootstrap_token::Column::Id.eq::<Uuid>(id.into()))
            .filter(bootstrap_token::Column::UsedAt.is_null())
            .filter(bootstrap_token::Column::RevokedAt.is_null())
            .col_expr(
                bootstrap_token::Column::UsedAt,
                Expr::value(Value::ChronoDateTimeUtc(Some(now))),
            )
            .col_expr(
                bootstrap_token::Column::ConsumedByUserId,
                Expr::value(Value::Uuid(Some(user_uuid))),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(res.rows_affected == 1)
    }

    /// Revoke a bootstrap token by setting `revoked_at = now()`.
    ///
    /// No-op if the row no longer exists.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// repo.revoke(token.id).await?;
    /// ```
    async fn revoke(&self, id: BootstrapTokenId) -> RepoResult<()> {
        let Some(model) = bootstrap_token::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active = model.into_active_model();
        active.revoked_at = Set(Some(Utc::now().into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
