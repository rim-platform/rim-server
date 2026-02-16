use async_trait::async_trait;
use entity::user;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::{NewUser, User, UserId};
use rim_core::user::repository::UserRepository;
use sea_orm::{ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmUserRepository {
    pub db: DbConn,
}

impl SeaOrmUserRepository {
    pub fn new(db: DbConn) -> Self {
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

    async fn create_user(&self, user_data: NewUser) -> RepoResult<User> {
        let user = user::ActiveModel {
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
}
