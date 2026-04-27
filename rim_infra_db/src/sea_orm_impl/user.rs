use async_trait::async_trait;
use entity::user;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::{NewUser, UpdateProfilePatch, User, UserId};
use rim_core::user::repository::UserRepository;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};

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
            password_hash: Set(user_data.password.to_owned()),
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

    async fn delete_by_username(&self, username: &str) -> RepoResult<()> {
        let user = self.find_by_username(username).await?;
        let Some(user) = user else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        user::Entity::delete_by_id(user.id)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(())
    }

    async fn any_users_exist(&self) -> RepoResult<bool> {
        let model = user::Entity::find()
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.is_some())
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

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::new(RepoErrorKind::NotFound))?;

        let mut active = model.into_active_model();
        active.password_hash = Set(password_hash);
        active.updated_at = Set(chrono::Utc::now().into());
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
