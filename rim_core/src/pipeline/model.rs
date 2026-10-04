use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::model::ConversationId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(PipelineId);

#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub id: PipelineId,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTrigger,
    pub filter: Option<PipelineFilter>,
    pub actions: Vec<PipelineAction>,
    pub enabled: bool,
    pub cooldown_secs: Option<i32>,
    pub created_by: UserId,
    pub schema_version: i32,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineTrigger {
    pub kind: PipelineTriggerKind,
    pub conversation_id: Option<ConversationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineTriggerKind {
    NewMessage,
    MessageUpdated,
    MessageDeleted,
    ReactionsUpdated,
    MemberJoined,
    WorkspaceStateChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineFilter {
    pub conditions: Vec<FilterCondition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FilterCondition {
    MessageContains { pattern: String },
    MessageFromUser { user_id: UserId },
    ConversationIs { conversation_id: ConversationId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineAction {
    ExecuteCommand {
        command_id: String,
        args: Option<serde_json::Value>,
    },
}
