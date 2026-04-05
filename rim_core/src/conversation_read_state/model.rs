use crate::conversation::model::ConversationId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationReadState {
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub last_read_at: DateTime<Utc>,
}
