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
pub struct ListServerMacrosQuery {
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
    /// Confirmation proof for any dangerous step inside the macro. Phase 1
    /// note: macros are an "all-or-nothing" confirmation surface — one
    /// proof issued at macro-issue time covers every nested dangerous
    /// step. The H1 args-binding still applies per nested call: a proof
    /// minted for macro M against the macro's input args binds to M's
    /// input args, NOT to the args of every step. Phase 4 may split this
    /// into per-step proofs once the modal can render a step list.
    pub confirmation: Option<ConfirmationProof>,
    /// Who triggered the macro. Propagated to every nested
    /// `ExecuteCommandCmd` produced by this macro's expansion — see
    /// [`crate::command::initiator::CommandInitiator`] docstring on
    /// `Macro` for the propagation rule. Defaults to `Palette` for
    /// back-compat.
    pub initiator: CommandInitiator,
}
