use async_trait::async_trait;
use entity::password_reset_token;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::password_reset::model::{
    NewPasswordResetToken, PasswordResetToken, PasswordResetTokenId,
};
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;

pub struct SeaOrmPasswordResetTokenRepository {
    db: DbHandle,
}

impl SeaOrmPasswordResetTokenRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl PasswordResetTokenRepository for SeaOrmPasswordResetTokenRepository {
    async fn create(&self, token: NewPasswordResetToken) -> RepoResult<PasswordResetToken> {
        let active_model = password_reset_token::ActiveModel {
            id: Set(PasswordResetTokenId::new().into()),
            user_id: Set(token.user_id.into()),
            token_hash: Set(token.token_hash),
            expires_at: Set(token.expires_at.into()),
            ..Default::default()
        };

        let res = password_reset_token::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PasswordResetToken>> {
        let model = password_reset_token::Entity::find()
            .filter(password_reset_token::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn try_mark_used(&self, id: PasswordResetTokenId) -> RepoResult<bool> {
        let now = chrono::Utc::now();
        let res = password_reset_token::Entity::update_many()
            .col_expr(password_reset_token::Column::UsedAt, Expr::value(now))
            .filter(password_reset_token::Column::Id.eq(sea_orm::prelude::Uuid::from(id)))
            .filter(password_reset_token::Column::UsedAt.is_null())
            .filter(password_reset_token::Column::ExpiresAt.gt(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected == 1)
    }

    async fn invalidate_unused_for_user(&self, user_id: UserId) -> RepoResult<u64> {
        let res = password_reset_token::Entity::update_many()
            .col_expr(
                password_reset_token::Column::UsedAt,
                Expr::value(chrono::Utc::now()),
            )
            .filter(password_reset_token::Column::UserId.eq(sea_orm::prelude::Uuid::from(user_id)))
            .filter(password_reset_token::Column::UsedAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}
