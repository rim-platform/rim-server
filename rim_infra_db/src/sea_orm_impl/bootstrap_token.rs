use async_trait::async_trait;
use chrono::Utc;
use entity::bootstrap_token;
use sea_orm::Value;
use sea_orm::prelude::{Expr, Uuid};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};

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

    /// Atomic revoke that returns the number of affected rows.
    ///
    /// `1` means this caller performed the revoke; `0` means the row was
    /// already revoked or does not exist. This is the implementation core of
    /// [`BootstrapTokenRepository::revoke`] — exposed separately so the
    /// concurrent-revoke regression test can assert that exactly one of two
    /// racing callers observes the "did revoke" outcome.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let did_revoke = repo.revoke_returning_affected(token.id).await? == 1;
    /// ```
    pub async fn revoke_returning_affected(&self, id: BootstrapTokenId) -> RepoResult<u64> {
        let now = Utc::now();
        let res = bootstrap_token::Entity::update_many()
            .filter(bootstrap_token::Column::Id.eq::<Uuid>(id.into()))
            .filter(bootstrap_token::Column::RevokedAt.is_null())
            .col_expr(
                bootstrap_token::Column::RevokedAt,
                Expr::value(Value::ChronoDateTimeUtc(Some(now))),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
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
    /// Executes a single
    /// `UPDATE bootstrap_token SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`
    /// statement, eliminating the read-then-update race where two concurrent
    /// admin revokes could each see `revoked_at IS NULL`, both write, and the
    /// second silently overwrites the first revoker's timestamp (C1-class
    /// hardening).
    ///
    /// Idempotent: a no-op when the row is already revoked or does not exist.
    /// Both outcomes are folded into `Ok(())` to keep the trait contract stable
    /// (matches `InviteRepository::revoke_invite` semantics). Test coverage uses
    /// the private [`Self::revoke_returning_affected`] helper to observe the
    /// "did revoke / already revoked" distinction.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// repo.revoke(token.id).await?;
    /// ```
    async fn revoke(&self, id: BootstrapTokenId) -> RepoResult<()> {
        let _ = self.revoke_returning_affected(id).await?;
        Ok(())
    }
}
