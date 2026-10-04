use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rim_core::conversation::model::ConversationId;
use rim_core::pipeline::model::PipelineId;
use rim_core::pipeline::model::{
    FilterCondition, Pipeline, PipelineAction, PipelineFilter, PipelineTrigger, PipelineTriggerKind,
};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineTriggerKindDto {
    NewMessage,
    MessageUpdated,
    MessageDeleted,
    ReactionsUpdated,
    MemberJoined,
    WorkspaceStateChanged,
}

impl From<PipelineTriggerKind> for PipelineTriggerKindDto {
    fn from(k: PipelineTriggerKind) -> Self {
        match k {
            PipelineTriggerKind::NewMessage => Self::NewMessage,
            PipelineTriggerKind::MessageUpdated => Self::MessageUpdated,
            PipelineTriggerKind::MessageDeleted => Self::MessageDeleted,
            PipelineTriggerKind::ReactionsUpdated => Self::ReactionsUpdated,
            PipelineTriggerKind::MemberJoined => Self::MemberJoined,
            PipelineTriggerKind::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

impl From<PipelineTriggerKindDto> for PipelineTriggerKind {
    fn from(k: PipelineTriggerKindDto) -> Self {
        match k {
            PipelineTriggerKindDto::NewMessage => Self::NewMessage,
            PipelineTriggerKindDto::MessageUpdated => Self::MessageUpdated,
            PipelineTriggerKindDto::MessageDeleted => Self::MessageDeleted,
            PipelineTriggerKindDto::ReactionsUpdated => Self::ReactionsUpdated,
            PipelineTriggerKindDto::MemberJoined => Self::MemberJoined,
            PipelineTriggerKindDto::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineTriggerDto {
    pub kind: PipelineTriggerKindDto,
    pub conversation_id: Option<ConversationId>,
}

impl From<PipelineTrigger> for PipelineTriggerDto {
    fn from(t: PipelineTrigger) -> Self {
        Self {
            kind: t.kind.into(),
            conversation_id: t.conversation_id,
        }
    }
}

impl From<PipelineTriggerDto> for PipelineTrigger {
    fn from(t: PipelineTriggerDto) -> Self {
        Self {
            kind: t.kind.into(),
            conversation_id: t.conversation_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterConditionDto {
    MessageContains { pattern: String },
    MessageFromUser { user_id: UserId },
    ConversationIs { conversation_id: ConversationId },
}

impl From<FilterCondition> for FilterConditionDto {
    fn from(c: FilterCondition) -> Self {
        match c {
            FilterCondition::MessageContains { pattern } => Self::MessageContains { pattern },
            FilterCondition::MessageFromUser { user_id } => Self::MessageFromUser { user_id },
            FilterCondition::ConversationIs { conversation_id } => {
                Self::ConversationIs { conversation_id }
            }
        }
    }
}

impl From<FilterConditionDto> for FilterCondition {
    fn from(c: FilterConditionDto) -> Self {
        match c {
            FilterConditionDto::MessageContains { pattern } => Self::MessageContains { pattern },
            FilterConditionDto::MessageFromUser { user_id } => Self::MessageFromUser { user_id },
            FilterConditionDto::ConversationIs { conversation_id } => {
                Self::ConversationIs { conversation_id }
            }
        }
    }
}

impl From<PipelineFilterDto> for PipelineFilter {
    fn from(f: PipelineFilterDto) -> Self {
        Self {
            conditions: f.conditions.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<PipelineActionDto> for PipelineAction {
    fn from(a: PipelineActionDto) -> Self {
        match a {
            PipelineActionDto::ExecuteCommand { command_id, args } => {
                Self::ExecuteCommand { command_id, args }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineFilterDto {
    pub conditions: Vec<FilterConditionDto>,
}

impl From<PipelineFilter> for PipelineFilterDto {
    fn from(f: PipelineFilter) -> Self {
        Self {
            conditions: f.conditions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PipelineActionDto {
    ExecuteCommand {
        command_id: String,
        args: Option<serde_json::Value>,
    },
}

impl From<PipelineAction> for PipelineActionDto {
    fn from(a: PipelineAction) -> Self {
        match a {
            PipelineAction::ExecuteCommand { command_id, args } => {
                Self::ExecuteCommand { command_id, args }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineDto {
    pub id: PipelineId,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerDto,
    pub filter: Option<PipelineFilterDto>,
    pub actions: Vec<PipelineActionDto>,
    pub enabled: bool,
    pub cooldown_secs: Option<i32>,
    pub created_by: UserId,
    pub schema_version: i32,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Pipeline> for PipelineDto {
    fn from(p: Pipeline) -> Self {
        Self {
            id: p.id,
            server_id: p.server_id,
            name: p.name,
            description: p.description,
            trigger: p.trigger.into(),
            filter: p.filter.map(Into::into),
            actions: p.actions.into_iter().map(Into::into).collect(),
            enabled: p.enabled,
            cooldown_secs: p.cooldown_secs,
            created_by: p.created_by,
            schema_version: p.schema_version,
            last_fired_at: p.last_fired_at,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineFiredResult {
    pub pipeline_id: String,
    pub pipeline_name: String,
    pub actions_executed: usize,
    pub skipped_reason: Option<String>,
}
