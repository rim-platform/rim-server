use crate::conversation::model::ConversationId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(ChannelLensId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelLens {
    pub id: ChannelLensId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub created_by: UserId,
    pub name: String,
    pub filter_spec: FilterSpec,
    pub is_shared: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FilterSpec {
    pub conditions: Vec<FilterCondition>,
    pub logic: FilterLogic,
}

impl Default for FilterSpec {
    fn default() -> Self {
        Self {
            conditions: Vec::new(),
            logic: FilterLogic::And,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FilterCondition {
    pub field: FilterField,
    pub op: FilterOp,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterField {
    HasLinks,
    AuthorId,
    HasAttachments,
    TextContains,
    IsPinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterOp {
    Eq,
    In,
    Contains,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterLogic {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct NewChannelLens {
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub created_by: UserId,
    pub name: String,
    pub filter_spec: FilterSpec,
    pub is_shared: bool,
}
