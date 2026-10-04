use crate::Actor;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MyMentionsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub limit: u32,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetMentionsForMessagesQuery {
    pub message_ids: Vec<MessageId>,
}
