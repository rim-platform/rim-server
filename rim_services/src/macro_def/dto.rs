use rim_core::macro_def::model::MacroId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use chrono::{DateTime, Utc};

use crate::command::sensitivity::CommandSensitivityDto;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MacroStepDto {
    pub command_id: String,
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MacroDto {
    pub id: MacroId,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStepDto>,
    pub ai_callable: bool,
    pub nl_description: String,
    pub intent_samples: Vec<String>,
    pub sensitivity: CommandSensitivityDto,
    pub created_at: DateTime<Utc>,
}
