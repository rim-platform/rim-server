use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::command::dto::{CommandDefinitionDto, CommandResultDto, UserCommandDto};
use crate::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, ListCommandsQuery,
    UpdateUserCommandCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CommandService: Send + Sync {
    async fn list_commands(
        &self,
        query: ListCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>>;
    async fn execute_command(&self, cmd: ExecuteCommandCmd) -> ServiceResult<CommandResultDto>;
    async fn create_user_command(&self, cmd: CreateUserCommandCmd)
    -> ServiceResult<UserCommandDto>;
    async fn update_user_command(&self, cmd: UpdateUserCommandCmd)
    -> ServiceResult<UserCommandDto>;
    async fn delete_user_command(&self, cmd: DeleteUserCommandCmd) -> ServiceResult<()>;
}
