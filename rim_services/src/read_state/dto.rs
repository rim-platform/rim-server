use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;

use crate::Actor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadCountDto {
    pub conversation_id: ConversationId,
    pub count: i64,
    /// Messages sent after this time count as unread.
    pub unread_after: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MarkAsReadCmd {
    pub actor: Actor,
    pub workspace_id: ServerId,
    pub conversation_id: ConversationId,
    pub read_at: DateTime<Utc>,
}

/// Marks a conversation unread from `message_id` on.
#[derive(Debug, Clone)]
pub struct MarkAsUnreadCmd {
    pub actor: Actor,
    pub workspace_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct MarkAllAsReadCmd {
    pub actor: Actor,
    pub workspace_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct UnreadCountsQuery {
    pub actor: Actor,
    pub workspace_id: ServerId,
}
