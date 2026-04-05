use rim_core::conversation::model::ConversationId;
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

    pub dir: PageDir,
    pub cursor: Option<ChannelPageCursor>,
    pub limit: u32,
}

/// These are typically valid only for Group conversations.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AddConversationMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub user_id: UserId,
}

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
