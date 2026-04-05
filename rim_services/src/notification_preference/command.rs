use crate::Actor;
use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MuteConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UnmuteConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListMutedConversationsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IsMutedQuery {
    pub actor: Actor,
    pub conversation_id: ConversationId,
}
