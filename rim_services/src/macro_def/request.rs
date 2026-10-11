use crate::Actor;
use crate::command::confirmation::ConfirmationProof;
use crate::command::dto::CommandContextDto;
use crate::command::initiator::CommandInitiator;
use crate::command::sensitivity::CommandSensitivityDto;
use crate::macro_def::dto::MacroStepDto;
use rim_core::macro_def::model::MacroId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct CreateMacroCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStepDto>,
    pub ai_callable: bool,
    pub nl_description: String,
    pub intent_samples: Vec<String>,
    pub sensitivity: CommandSensitivityDto,
}

#[derive(Debug, Clone)]
pub struct UpdateMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStepDto>,
    pub ai_callable: bool,
    pub nl_description: String,
    pub intent_samples: Vec<String>,
    pub sensitivity: CommandSensitivityDto,
}

#[derive(Debug, Clone)]
pub struct DeleteMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
}

#[derive(Debug, Clone)]
pub struct ListUserMacrosQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct GetMacroByNameQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SynthesizeMacroCommandsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct ExecuteMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
    pub command_context: Option<CommandContextDto>,
    /// Confirmation proof for the macro. Macros are an all-or-nothing
    /// confirmation surface: one proof, bound to the macro's own input args
    /// (not the args of each step), covers every dangerous step. Once it
    /// verifies, the server issues each step its own single-use token.
    pub confirmation: Option<ConfirmationProof>,
    /// Who triggered the macro. Propagated to every nested
    /// `ExecuteCommandCmd` produced by this macro's expansion — see
    /// [`crate::command::initiator::CommandInitiator`] docstring on
    /// `Macro` for the propagation rule. The GraphQL layer fills in
    /// `Palette` when the client sends none.
    pub initiator: CommandInitiator,
}
