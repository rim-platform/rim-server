use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::{ConversationActivityCursor, ConversationCreatedCursor};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use rim_core::conversation::model::{ConversationType, PostingPolicy};

#[derive(Debug, Clone)]
pub struct ConversationDto {
    pub id: ConversationId,
    pub server_id: ServerId,
    pub conversation_type: ConversationType,

    pub title: Option<String>,
    pub topic: Option<String>,

    /// Active members: they read, send and are notified.
    pub member_ids: Vec<UserId>,
    /// Members of an encrypted conversation who wait for a device ready for encryption.
    pub pending_member_ids: Vec<UserId>,

    pub is_private: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<UserId>,
    /// Slow-mode interval in seconds, `0` when off.
    pub slow_mode_seconds: i32,
    /// When the conversation became end-to-end encrypted.
    pub encrypted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Who may post in the channel; `Everyone` for DMs and groups.
    pub posting_policy: PostingPolicy,
    /// Under `PostingPolicy::Moderators`, whether everyone else may still react and reply in
    /// threads.
    pub posting_allows_threads: bool,
}

impl ConversationDto {
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn is_member(&self, user_id: UserId) -> bool {
        self.member_ids.contains(&user_id)
    }

    pub fn member_count(&self) -> usize {
        self.member_ids.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InboxPageCursor {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: ConversationId,
}

impl From<ConversationActivityCursor> for InboxPageCursor {
    fn from(value: ConversationActivityCursor) -> Self {
        Self {
            last_activity_at: value.last_activity_at,
            conversation_id: value.conversation_id,
        }
    }
}

impl From<InboxPageCursor> for ConversationActivityCursor {
    fn from(value: InboxPageCursor) -> Self {
        Self {
            last_activity_at: value.last_activity_at,
            conversation_id: value.conversation_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChannelPageCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: ConversationId,
}

impl From<ConversationCreatedCursor> for ChannelPageCursor {
    fn from(value: ConversationCreatedCursor) -> Self {
        Self {
            created_at: value.created_at,
            conversation_id: value.conversation_id,
        }
    }
}

impl From<ChannelPageCursor> for ConversationCreatedCursor {
    fn from(value: ChannelPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            conversation_id: value.conversation_id,
        }
    }
}
