use async_trait::async_trait;
use entity::user;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::{NewUser, UpdateProfilePatch, User, UserId};
use rim_core::user::repository::UserRepository;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, sea_query::Expr,
};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmUserRepository {
    pub db: DbHandle,
}

impl SeaOrmUserRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserRepository for SeaOrmUserRepository {
    async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_by_ids(&self, ids: &[UserId]) -> RepoResult<Vec<User>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<sea_orm::prelude::Uuid> = ids.iter().copied().map(Into::into).collect();
        let models = user::Entity::find()
            .filter(user::Column::Id.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create_user(&self, user_data: NewUser) -> RepoResult<User> {
        let user = user::ActiveModel {
            id: Set(UserId::new().into()),
            username: Set(user_data.username.to_owned()),
            password_hash: Set(user_data.password),
            email: Set(user_data.email),
            display_name: Set(user_data.display_name),
            principal_id: Set(user_data
                .key_identity
                .as_ref()
                .map(|k| k.principal_id.clone())),
            identity_key: Set(user_data.key_identity.map(|k| k.identity_key)),
            ..Default::default()
        };
        let res = user::Entity::insert(user)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?
            .into();
        Ok(res)
    }

    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>> {
        let model = user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update_profile(&self, id: UserId, patch: UpdateProfilePatch) -> RepoResult<User> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::new(RepoErrorKind::NotFound))?;

        let mut active = model.into_active_model();

        if let Some(val) = patch.display_name {
            active.display_name = Set(val);
        }
        if let Some(val) = patch.avatar_url {
            active.avatar_url = Set(val);
        }
        if let Some(val) = patch.status_text {
            active.status_text = Set(val);
        }
        if let Some(val) = patch.bio {
            active.bio = Set(val);
        }

        active.updated_at = Set(chrono::Utc::now().into());

        let updated = user::Entity::update(active)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(updated.into())
    }

    /// Find a user by email address using a case-insensitive comparison.
    ///
    /// Applies `LOWER(email) = LOWER($1)` so that input casing is irrelevant.
    ///
    /// # Errors
    /// Propagates any underlying database error as [`rim_core::repository::error::RepoError`].
    async fn find_by_email(&self, email: &str) -> RepoResult<Option<User>> {
        let lower_email = email.to_lowercase();
        // Use a custom SQL expression: LOWER(email) = $1
        // This mirrors the idx_user_email_lower_unique index on the schema layer.
        let model = user::Entity::find()
            .filter(Expr::cust_with_values(
                "LOWER(email) = $1",
                [sea_orm::Value::from(lower_email)],
            ))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::new(RepoErrorKind::NotFound))?;

        let mut active = model.into_active_model();
        active.password_hash = Set(Some(password_hash));
        active.updated_at = Set(chrono::Utc::now().into());
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
