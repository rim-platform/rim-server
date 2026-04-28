use async_trait::async_trait;
use chrono::Utc;
use entity::bootstrap_token;
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

    /// Mark a bootstrap token as used, recording the consuming user.
    ///
    /// Sets `used_at = now()` and `consumed_by_user_id`. No-op if the row
    /// no longer exists.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// repo.mark_used(token.id, user.id).await?;
    /// ```
    async fn mark_used(&self, id: BootstrapTokenId, consumed_by_user_id: UserId) -> RepoResult<()> {
        let Some(model) = bootstrap_token::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active = model.into_active_model();
        active.used_at = Set(Some(Utc::now().into()));
        active.consumed_by_user_id = Set(Some(consumed_by_user_id.into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
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
