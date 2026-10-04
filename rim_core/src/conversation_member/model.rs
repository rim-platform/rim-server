use chrono::{DateTime, Utc};

use crate::conversation::model::ConversationId;
use crate::model_id;
use crate::user::model::UserId;

model_id!(ConversationMemberId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMember {
    pub id: ConversationMemberId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub joined_at: DateTime<Utc>,
}
