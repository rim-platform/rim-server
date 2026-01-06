use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::user;
use rim_core::repository::repository::Repository;
use rim_core::user::model::{NewUser, User, UserId};
use rim_core::user::repository::UserRepository;
use sea_orm::{ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

pub struct SeaOrmUserRepository {
    pub db: DbConn,
}

impl SeaOrmUserRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(SeaOrmUserRepository, user::Entity, User, UserId);

#[async_trait]
impl UserRepository for SeaOrmUserRepository {
    async fn create_user(&self, user_data: NewUser) -> anyhow::Result<Self::Model> {
        let user = user::ActiveModel {
            username: Set(user_data.username.to_owned()),
            password_hash: Set(user_data.password.to_owned()),
            ..Default::default()
        };
        let res = user::Entity::insert(user)
            .exec_with_returning(&self.db)
            .await?
            .into();
        Ok(res)
    }

    async fn find_by_username(&self, username: &str) -> anyhow::Result<Option<Self::Model>> {
        let model = user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(&self.db)
            .await?;

        Ok(model.map(Into::into))
    }

    async fn delete_by_username(&self, username: &str) -> anyhow::Result<()> {
        if let Some(user) = self.find_by_username(username).await? {
            self.delete_by_id(user.id).await?;
        }

        Ok(())
    }
}
