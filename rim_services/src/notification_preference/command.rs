use crate::Actor;
use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct MuteConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct UnmuteConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct ListMutedConversationsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct IsMutedQuery {
    pub actor: Actor,
    pub conversation_id: ConversationId,
}
