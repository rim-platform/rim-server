use async_trait::async_trait;
use chrono::Utc;
use entity::invite;
use sea_orm::Value;
use sea_orm::prelude::{Expr, Uuid};
use sea_orm::{ColumnTrait, EntityTrait, ExprTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::auth::model::{Invite, InviteId, NewInvite};
use rim_core::auth::repository::InviteRepository;
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;

/// SeaORM-backed implementation of [`InviteRepository`] (workspace invites).
pub struct SeaOrmInviteRepository {
    db: DbHandle,
}

impl SeaOrmInviteRepository {
    /// Construct a new repository using the given [`DbHandle`].
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    /// Atomic revoke that returns the number of affected rows.
    ///
    /// `1` means this caller performed the revoke; `0` means the row was
    /// already revoked or does not exist. This is the implementation core of
    /// [`InviteRepository::revoke_invite`] — exposed separately so the
    /// concurrent-revoke regression test can assert that exactly one of two
    /// racing callers observes the "did revoke" outcome.
    ///
    /// This is the workspace-invite analog of
    /// [`SeaOrmBootstrapTokenRepository::revoke_returning_affected`] and
    /// [`SeaOrmInstanceInviteRepository::revoke_returning_affected`] — the
    /// same C1-class race-closure pattern applied to the third revoke path.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// let did_revoke = repo.revoke_invite_returning_affected(invite.id).await? == 1;
    /// ```
    ///
    /// [`SeaOrmBootstrapTokenRepository::revoke_returning_affected`]: crate::sea_orm_impl::bootstrap_token::SeaOrmBootstrapTokenRepository::revoke_returning_affected
    /// [`SeaOrmInstanceInviteRepository::revoke_returning_affected`]: crate::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository::revoke_returning_affected
    pub async fn revoke_invite_returning_affected(&self, id: InviteId) -> RepoResult<u64> {
        let now = Utc::now();
        let res = invite::Entity::update_many()
            .filter(invite::Column::Id.eq::<Uuid>(id.into()))
            .filter(invite::Column::RevokedAt.is_null())
            .col_expr(
                invite::Column::RevokedAt,
                Expr::value(Value::ChronoDateTimeUtc(Some(now))),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}

#[async_trait]
impl InviteRepository for SeaOrmInviteRepository {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>> {
        let model = invite::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>> {
        let model = invite::Entity::find()
            .filter(invite::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn create_invite(&self, inv: NewInvite) -> RepoResult<Invite> {
        let active_model = invite::ActiveModel {
            id: Set(InviteId::new().into()),
            server_id: Set(inv.server_id.into()),
            created_by_user_id: Set(inv.created_by_user_id.into()),
            token_hash: Set(inv.token_hash),
            expires_at: Set(inv.expires_at.map(Into::into)),
            max_uses: Set(inv.max_uses),
            ..Default::default()
        };

        let res = invite::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    async fn try_consume(&self, id: InviteId) -> RepoResult<bool> {
        // Atomic update: increment use_count only if below max_uses.
        let res =
            invite::Entity::update_many()
                .filter(invite::Column::Id.eq::<Uuid>(id.into()))
                .filter(sea_orm::Condition::all().add(
                    Expr::col(invite::Column::UseCount).lt(Expr::col(invite::Column::MaxUses)),
                ))
                .col_expr(
                    invite::Column::UseCount,
                    Expr::col(invite::Column::UseCount).add(1),
                )
                .exec(&self.db)
                .await
                .map_err(map_db_err)?;

        Ok(res.rows_affected == 1)
    }

    /// Revoke a workspace invite by setting `revoked_at = now()`.
    ///
    /// Executes a single
    /// `UPDATE invite SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`
    /// statement, eliminating the read-then-update race where two concurrent
    /// admin revokes could each see `revoked_at IS NULL`, both write, and the
    /// second silently overwrites the first revoker's timestamp (C1-class
    /// hardening — workspace-invite analog of the same fix applied to
    /// [`SeaOrmBootstrapTokenRepository::revoke`] and
    /// [`SeaOrmInstanceInviteRepository::revoke`]).
    ///
    /// Idempotent: a no-op when the row is already revoked or does not exist.
    /// Both outcomes are folded into `Ok(())` to keep the trait contract stable.
    /// Test coverage uses the inherent
    /// [`Self::revoke_invite_returning_affected`] helper to observe the
    /// "did revoke / already revoked" distinction.
    ///
    /// # Errors
    /// Returns a repository error on database failure.
    ///
    /// # Examples
    /// ```ignore
    /// repo.revoke_invite(invite.id).await?;
    /// ```
    ///
    /// [`SeaOrmBootstrapTokenRepository::revoke`]: crate::sea_orm_impl::bootstrap_token::SeaOrmBootstrapTokenRepository
    /// [`SeaOrmInstanceInviteRepository::revoke`]: crate::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository
    async fn revoke_invite(&self, id: InviteId) -> RepoResult<()> {
        let _ = self.revoke_invite_returning_affected(id).await?;
        Ok(())
    }

    async fn list_active_by_server(&self, server_id: ServerId) -> RepoResult<Vec<Invite>> {
        let models = invite::Entity::find()
            .filter(invite::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(invite::Column::RevokedAt.is_null())
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }
}
