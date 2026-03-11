use chrono::{DateTime, Utc};

use crate::model_id;
use crate::user::model::UserId;

model_id!(UserCommandId);

// This domain currently models persisted user-authored commands/macros. Built-in, plugin, and
// integration command definitions live in higher layers until a broader command catalog is needed.

#[derive(Debug, Clone, PartialEq)]
pub struct UserCommand {
    pub id: UserCommandId,
    pub owner_user_id: UserId,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps_json: serde_json::Value,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewUserCommand {
    pub owner_user_id: UserId,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps_json: serde_json::Value,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserCommand {
    pub title: String,
    pub description: String,
    pub steps_json: serde_json::Value,
    pub is_enabled: bool,
}

pub fn is_user_command_key(command_key: &str) -> bool {
    command_key.starts_with("user.")
}
