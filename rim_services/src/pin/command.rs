use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

use crate::Actor;

#[derive(Debug, Clone)]
pub struct PinMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct UnpinMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct ListPinnedMessagesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}
