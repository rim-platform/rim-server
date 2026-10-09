use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct ToggleReactionCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub emoji: String,
}

#[derive(Debug, Clone, Copy)]
pub struct GetMessageReactionsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct GetReactionsForMessagesQuery {
    pub actor: Actor,
    pub message_ids: Vec<MessageId>,
}
