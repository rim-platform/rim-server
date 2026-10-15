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
    /// Set while the member waits for a device that can join the encrypted conversation.
    pub pending_since: Option<DateTime<Utc>>,
}

/// A user's standing in a conversation they belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    /// Reads, sends and is notified.
    Active,
    /// Listed, but cannot read, send or be notified until a device of theirs is ready.
    Pending,
}
