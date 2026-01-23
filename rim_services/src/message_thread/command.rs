use crate::Actor;
use crate::message_thread::dto::ThreadPageCursor;
use rim_core::conversation::model::ConversationId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetThreadQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub thread_id: MessageThreadId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListThreadsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub dir: PageDir,
    pub cursor: Option<ThreadPageCursor>,
    pub limit: u32,
}
