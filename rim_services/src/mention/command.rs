use crate::Actor;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct MyMentionsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct GetMentionsForMessagesQuery {
    pub message_ids: Vec<MessageId>,
}
