use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::message::model::MessageId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ThreadDto {
    pub id: MessageThreadId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub parent_message_id: MessageId,
    pub created_by: UserId,

    pub title: Option<String>,
}
