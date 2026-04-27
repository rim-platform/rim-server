use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::command::dto::{CommandDefinitionDto, CommandResultDto};
use crate::macro_def::dto::MacroDto;
use crate::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, GetMacroByNameQuery, ListServerMacrosQuery,
    ListUserMacrosQuery, SynthesizeMacroCommandsQuery, UpdateMacroCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MacroService: Send + Sync {
    async fn create_macro(&self, cmd: CreateMacroCmd) -> ServiceResult<MacroDto>;
    async fn update_macro(&self, cmd: UpdateMacroCmd) -> ServiceResult<MacroDto>;
    async fn delete_macro(&self, cmd: DeleteMacroCmd) -> ServiceResult<()>;
    async fn get_macro_by_name(
        &self,
        query: GetMacroByNameQuery,
    ) -> ServiceResult<Option<MacroDto>>;
    async fn list_user_macros(&self, query: ListUserMacrosQuery) -> ServiceResult<Vec<MacroDto>>;
    async fn list_server_macros(
        &self,
        query: ListServerMacrosQuery,
    ) -> ServiceResult<Vec<MacroDto>>;
    async fn execute_macro(&self, cmd: ExecuteMacroCmd) -> ServiceResult<CommandResultDto>;
    /// Return `CommandDefinitionDto` representations of all AI-callable macros
    /// owned by the actor in the given server. Used by the command discovery
    /// surface to expose user macros to the AI agent tool list.
    async fn synthesize_command_definitions(
        &self,
        query: SynthesizeMacroCommandsQuery,
    ) -> ServiceResult<Vec<CommandDefinitionDto>>;
}
