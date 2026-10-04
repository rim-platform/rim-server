use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;

use crate::Actor;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnreadCountDto {
    pub conversation_id: ConversationId,
    pub count: i64,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MarkAsReadCmd {
    pub actor: Actor,
    pub workspace_id: ServerId,
    pub conversation_id: ConversationId,
    pub read_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MarkAllAsReadCmd {
    pub actor: Actor,
    pub workspace_id: ServerId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnreadCountsQuery {
    pub actor: Actor,
    pub workspace_id: ServerId,
}
