use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::ChannelListFilter;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::Actor;
use crate::conversation::dto::{ChannelPageCursor, InboxPageCursor};

#[derive(Debug, Clone)]
pub struct CreateConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,

    pub participants: crate::conversation::types::ConversationParticipants,

    pub title: Option<String>,

    /// Channels only: visible to members alone. Rejected with `Validation` for DMs and groups.
    /// Cannot be changed after creation.
    pub is_private: bool,

    /// DMs and groups only: end-to-end encrypted from creation. Every member must be ready
    /// for encryption.
    pub encrypted: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GetConversationQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, Copy)]
pub struct ListInboxConversationsQuery {
    pub actor: Actor,
    pub server_id: ServerId,

    pub dir: PageDir,
    pub cursor: Option<InboxPageCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ListChannelsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub filter: ChannelListFilter,

    pub dir: PageDir,
    pub cursor: Option<ChannelPageCursor>,
    pub limit: u32,
}

/// Add (invite) workspace members to a channel or group. The actor must be a member of the
/// conversation. Not valid for DMs.
#[derive(Debug, Clone)]
pub struct AddConversationMembersCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub user_ids: Vec<UserId>,
}

/// Remove a member from a channel or group: Owner/Moderator may remove anyone, anyone else only
/// themselves (equivalent to [`LeaveConversationCmd`]).
#[derive(Debug, Clone)]
pub struct RemoveConversationMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
pub struct UpdateConversationTopicCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub topic: Option<String>,
}

/// Join a public, unarchived channel. Idempotent.
#[derive(Debug, Clone, Copy)]
pub struct JoinChannelCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

/// Leave a channel or group (not a DM).
#[derive(Debug, Clone, Copy)]
pub struct LeaveConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

/// Accept or decline the caller's pending invitation to an encrypted conversation.
#[derive(Debug, Clone)]
pub struct ConversationInviteCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

/// Archive (`archived: true`) or unarchive a channel. Owner/Moderator only.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub archived: bool,
}
