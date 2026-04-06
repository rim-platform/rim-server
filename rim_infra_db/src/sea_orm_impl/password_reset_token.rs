use async_trait::async_trait;
use entity::password_reset_token;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::password_reset::model::{
    NewPasswordResetToken, PasswordResetToken, PasswordResetTokenId,
};
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::repository::error::RepoResult;

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

    async fn mark_used(&self, id: PasswordResetTokenId) -> RepoResult<()> {
        let model = password_reset_token::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(model) = model else {
            return Ok(());
        };

        let mut active = model.into_active_model();
        active.used_at = Set(Some(chrono::Utc::now().into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
