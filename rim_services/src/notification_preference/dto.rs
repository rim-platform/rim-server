use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;

#[derive(Debug, Clone)]
pub struct MutedConversationDto {
    pub conversation_id: ConversationId,
    pub muted_until: Option<DateTime<Utc>>,
}
