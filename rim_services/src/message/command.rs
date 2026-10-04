use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;

use crate::message::dto::MessagePageCursor;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplyToMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub root_message_id: MessageId,
    pub text: String,
    pub parent_message_id: Option<MessageId>,
    pub attachments: Vec<MessageAttachmentRef>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SendMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub text: Option<String>,

    pub parent_message_id: Option<MessageId>,

    pub attachments: Vec<MessageAttachmentRef>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageAttachmentRef {
    pub attachment_id: rim_core::message_attachment::model::MessageAttachmentId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetMessageQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetMessageReferenceQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListMessagesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub dir: PageDir,
    pub cursor: Option<MessagePageCursor>,
    pub limit: u32,
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListThreadMessagesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub thread_id: MessageThreadId,
    pub dir: PageDir,
    pub cursor: Option<MessagePageCursor>,
    pub limit: u32,
    pub include_deleted: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SearchMessagesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    /// Optional explicit single-conversation scope, in addition to the `in:` search operator.
    /// When set, the full member-access gate is run for this conversation.
    pub conversation_id: Option<ConversationId>,
    /// Raw search string — parsed by [`crate::message::search_query::parse_search_query`].
    pub query: String,
    pub limit: u32,
    pub cursor: Option<MessagePageCursor>,
    pub dir: PageDir,
    // `include_deleted` intentionally removed: search never returns deleted messages, matching
    // the forced behavior the old implementation already had. See `DefaultMessageService::search_messages`.
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EditMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,

    pub text: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ForwardMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub source_conversation_id: ConversationId,
    pub target_conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,

    pub hard: bool,
}
