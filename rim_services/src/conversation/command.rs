use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::ChannelListFilter;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::Actor;
use crate::conversation::dto::{ChannelPageCursor, InboxPageCursor};

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,

    pub participants: crate::conversation::types::ConversationParticipants,

    pub title: Option<String>,

    /// Channels only: visible to members alone. Rejected with `Validation` for DMs and groups.
    /// Cannot be changed after creation.
    pub is_private: bool,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetConversationQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListInboxConversationsQuery {
    pub actor: Actor,
    pub server_id: ServerId,

    pub dir: PageDir,
    pub cursor: Option<InboxPageCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AddConversationMembersCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub user_ids: Vec<UserId>,
}

/// Remove a member from a channel or group: Owner/Moderator may remove anyone, anyone else only
/// themselves (equivalent to [`LeaveConversationCmd`]).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoveConversationMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateConversationTopicCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub topic: Option<String>,
}

/// Join a public, unarchived channel. Idempotent.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JoinChannelCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

/// Leave a channel or group (not a DM).
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LeaveConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

/// Archive (`archived: true`) or unarchive a channel. Owner/Moderator only.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArchiveConversationCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub archived: bool,
}
