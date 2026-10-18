use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::server::model::ServerId;

use crate::Actor;
use crate::ServiceResult;
use crate::command::dto::{CommandDefinitionDto, CommandResultDto, UserCommandDto};
use crate::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    GetUserCommandQuery, ListCommandsQuery, ListUserCommandsQuery, UpdateUserCommandCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait CommandService: Send + Sync {
    async fn list_commands(
        &self,
        query: ListCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>>;
    async fn get_command(
        &self,
        query: GetCommandQuery,
    ) -> ServiceResult<Option<CommandDefinitionDto>>;
    async fn execute_command(&self, cmd: ExecuteCommandCmd) -> ServiceResult<CommandResultDto>;
    async fn create_user_command(&self, cmd: CreateUserCommandCmd)
    -> ServiceResult<UserCommandDto>;
    async fn update_user_command(&self, cmd: UpdateUserCommandCmd)
    -> ServiceResult<UserCommandDto>;
    async fn delete_user_command(&self, cmd: DeleteUserCommandCmd) -> ServiceResult<()>;

    /// One of the actor's own user commands with its stored steps in order, enabled or not.
    ///
    /// Returns `None` when the actor has no user command with that id, which covers an id
    /// outside the `user.` namespace and another user's command.
    ///
    /// # Errors
    ///
    /// `ServiceError::Internal` when the stored steps cannot be read; a repository failure as
    /// the `ServiceError` it maps to.
    async fn get_user_command(
        &self,
        query: GetUserCommandQuery,
    ) -> ServiceResult<Option<UserCommandDto>>;

    /// The actor's own user commands with their stored steps, enabled or not, sorted by
    /// command id.
    ///
    /// # Errors
    ///
    /// `ServiceError::Internal` when stored steps cannot be read; a repository failure as the
    /// `ServiceError` it maps to.
    async fn list_user_commands(
        &self,
        query: ListUserCommandsQuery,
    ) -> ServiceResult<Vec<UserCommandDto>>;

    /// The AI agent's command-discovery meta-tool.
    ///
    /// Returns the actor's accessible AI-callable command surface, optionally
    /// ranked by relevance to an `intent_phrase`, truncated to `limit`, and
    /// scoped to a workspace for macro synthesis.
    ///
    /// **Filtering pipeline (in order):**
    ///   1. `ai_callable: true` — non-AI commands are excluded.
    ///   2. Actor capability gate — commands whose `required_capabilities` the
    ///      actor does not hold are silently dropped (same gate as `Surface::Ai`
    ///      in `list_commands`).
    ///   3. Macro inclusion — when `workspace_id` is `Some`, synthesized macro
    ///      definitions are merged before the capability gate runs.
    ///
    /// **Intent ranking** (literal substring + token overlap):
    ///   - When `intent_phrase` is `Some`, commands are ranked descending by a
    ///     score computed from substring + token-overlap matches against
    ///     `intent_samples`, `title`, and `nl_description`.
    ///   - Tie-break: `command_id` ascending (deterministic for agents).
    ///   - When `intent_phrase` is `None`, output is sorted by `command_id`
    ///     ascending (same stable order as `list_commands`).
    ///
    /// **Result cap:** truncated to `limit.unwrap_or(50).min(100)`.
    ///
    /// **Rate limit:** 5 invocations per 60-second sliding window per actor, on
    /// the same single-node limiter as `execute_command`. Returns
    /// `ServiceError::RateLimited` on overage.
    async fn command_discover(
        &self,
        actor: &Actor,
        intent_phrase: Option<String>,
        limit: Option<u32>,
        workspace_id: Option<ServerId>,
    ) -> ServiceResult<Vec<CommandDefinitionDto>>;
}
