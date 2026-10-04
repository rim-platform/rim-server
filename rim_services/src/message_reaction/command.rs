use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToggleReactionCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub emoji: String,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetMessageReactionsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetReactionsForMessagesQuery {
    pub actor: Actor,
    pub message_ids: Vec<MessageId>,
}
