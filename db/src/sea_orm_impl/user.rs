use async_trait::async_trait;
use sea_orm::{DbConn, EntityTrait, QueryFilter, QueryOrder, Set};
use sea_orm::prelude::DateTimeWithTimeZone;
use entity::user;
use rim_core::repositories::user_repository::UserRepository;
use rim_core::repositories::repository::Repository;
use crate::impl_repository_for_entity;

pub struct UserData {
    pub username: String,
    pub password: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub last_login_at: DateTimeWithTimeZone,
}

pub struct SeaOrmUserRepository {
    pub db: DbConn,
}

impl SeaOrmUserRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(SeaOrmUserRepository, user::Model, user::Column, Id, user::Entity);

#[async_trait]
impl UserRepository<user::Model, Id, UserData> for SeaOrmUserRepository {
    async fn create_user(&self, user_data: UserData) -> anyhow::Result<user::Model> {
        let user = user::ActiveModel {
            username: Set(user_data.username.to_owned()),
            password_hash: Set(user_data.password.to_owned()),
            ..Default::default()
        };
        let res = user::Entity::insert(user).exec_with_returning(&self.db).await?;
        Ok(res)
    }

    async fn find_by_username(&self, username: &str) -> anyhow::Result<Option<user::Model>> {
        use sea_orm::{ColumnTrait};

        let model = user::Entity::find()
            .filter(user::Column::Username.eq(username))
            .one(&self.db)
            .await?;

        Ok(model)
    }

    async fn delete_by_username(&self, username: &str) -> anyhow::Result<()> {
        if let Some(user) = self.find_by_username(username).await? {
            self.delete_by_id(user.id).await?;
        }

        Ok(())
    }
}
