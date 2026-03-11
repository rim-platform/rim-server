use crate::Actor;
use crate::command::dto::{CommandContextDto, CommandSurfaceDto, UserCommandStepDto};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListCommandsQuery {
    pub actor: Actor,
    pub context: Option<CommandContextDto>,
    pub surface: CommandSurfaceDto,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExecuteCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    pub context: Option<CommandContextDto>,
    pub confirmed_dangerous: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateUserCommandCmd {
    pub actor: Actor,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
}
