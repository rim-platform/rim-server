use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone, Copy)]
pub struct GetThreadQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub thread_id: MessageThreadId,
}

#[derive(Debug, Clone, Copy)]
pub struct GetThreadByRootMessageQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub root_message_id: MessageId,
}
