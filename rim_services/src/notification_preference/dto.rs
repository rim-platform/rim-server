use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MutedConversationDto {
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
}
