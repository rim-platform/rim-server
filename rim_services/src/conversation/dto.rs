use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::conversation::types::ConversationType;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConversationDto {
    pub id: ConversationId,
    pub server_id: ServerId,
    pub conversation_type: ConversationType,

    pub title: Option<String>,

    pub member_ids: Vec<UserId>,
}
