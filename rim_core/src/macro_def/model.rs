use chrono::{DateTime, Utc};

use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(MacroId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroDef {
    pub id: MacroId,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub name: String,
    pub description: Option<String>,
    pub steps_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMacroDef {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub name: String,
    pub description: Option<String>,
    pub steps_json: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateMacroDef {
    pub name: String,
    pub description: Option<String>,
    pub steps_json: serde_json::Value,
}
