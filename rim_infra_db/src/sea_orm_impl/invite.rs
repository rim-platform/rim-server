use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::invite;
use sea_orm::ExprTrait;
use sea_orm::prelude::{Expr, Uuid};
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

use crate::error::map_db_err;
use rim_core::auth::model::{Invite, InviteId, NewInvite};
use rim_core::auth::repository::InviteRepository;
use rim_core::repository::error::RepoResult;

pub struct SeaOrmInviteRepository {
    db: DbConn,
}

impl SeaOrmInviteRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
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

    async fn revoke_invite(&self, id: InviteId) -> RepoResult<()> {
        let now: DateTime<Utc> = Utc::now();
        let Some(model) = invite::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
        else {
            return Ok(());
        };

        let mut active_model: invite::ActiveModel = model.into();
        active_model.revoked_at = Set(Some(now.into()));
        active_model.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
