use async_trait::async_trait;
use entity::user_command;
use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand, UserCommandId};
use rim_core::command::repository::UserCommandRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use rim_core::repository::error::{RepoError, RepoErrorKind};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmUserCommandRepository {
    db: DbHandle,
}

impl SeaOrmUserCommandRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserCommandRepository for SeaOrmUserCommandRepository {
    async fn get_by_id(&self, id: UserCommandId) -> RepoResult<Option<UserCommand>> {
        let model = user_command::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_key_for_user(
        &self,
        owner_user_id: UserId,
        command_key: &str,
    ) -> RepoResult<Option<UserCommand>> {
        let model = user_command::Entity::find()
            .filter(user_command::Column::OwnerUserId.eq::<Uuid>(owner_user_id.into()))
            .filter(user_command::Column::CommandKey.eq(command_key))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_for_user(&self, owner_user_id: UserId) -> RepoResult<Vec<UserCommand>> {
        let models = user_command::Entity::find()
            .filter(user_command::Column::OwnerUserId.eq::<Uuid>(owner_user_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create_user_command(&self, new_command: NewUserCommand) -> RepoResult<UserCommand> {
        let model = user_command::ActiveModel {
            id: Set(UserCommandId::new().into()),
            owner_user_id: Set(new_command.owner_user_id.into()),
            command_key: Set(new_command.command_key),
            title: Set(new_command.title),
            description: Set(new_command.description),
            steps_json: Set(new_command.steps_json),
            is_enabled: Set(new_command.is_enabled),
            ..Default::default()
        };

        let created = user_command::Entity::insert(model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(created.into())
    }

    async fn update_user_command(
        &self,
        id: UserCommandId,
        update: UpdateUserCommand,
    ) -> RepoResult<UserCommand> {
        let model = user_command::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: user_command::ActiveModel = model.into();
        active.title = Set(update.title);
        active.description = Set(update.description);
        active.steps_json = Set(update.steps_json);
        active.is_enabled = Set(update.is_enabled);
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete_user_command(&self, id: UserCommandId) -> RepoResult<()> {
        let model = user_command::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let active: user_command::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
