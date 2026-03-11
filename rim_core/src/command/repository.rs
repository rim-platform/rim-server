use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use crate::command::model::{NewUserCommand, UpdateUserCommand, UserCommand, UserCommandId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait UserCommandRepository: Send + Sync {
    async fn get_by_id(&self, id: UserCommandId) -> RepoResult<Option<UserCommand>>;

    async fn get_by_key_for_user(
        &self,
        owner_user_id: UserId,
        command_key: &str,
    ) -> RepoResult<Option<UserCommand>>;

    async fn list_for_user(&self, owner_user_id: UserId) -> RepoResult<Vec<UserCommand>>;

    async fn create_user_command(&self, new_command: NewUserCommand) -> RepoResult<UserCommand>;

    async fn update_user_command(
        &self,
        id: UserCommandId,
        update: UpdateUserCommand,
    ) -> RepoResult<UserCommand>;

    async fn delete_user_command(&self, id: UserCommandId) -> RepoResult<()>;
}
