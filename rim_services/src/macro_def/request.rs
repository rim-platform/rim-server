use crate::Actor;
use crate::command::dto::CommandContextDto;
use crate::macro_def::dto::MacroStepDto;
use rim_core::macro_def::model::MacroId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateMacroCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStepDto>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStepDto>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListUserMacrosQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListServerMacrosQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetMacroByNameQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExecuteMacroCmd {
    pub actor: Actor,
    pub macro_id: MacroId,
    pub command_context: Option<CommandContextDto>,
    pub confirmed_dangerous: bool,
}
