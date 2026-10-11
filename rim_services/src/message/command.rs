use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;

use crate::message::dto::MessagePageCursor;

#[derive(Debug, Clone)]
pub struct ReplyToMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub root_message_id: MessageId,
    pub text: String,
    pub parent_message_id: Option<MessageId>,
}

#[derive(Debug, Clone)]
pub struct SendMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub text: Option<String>,

    pub parent_message_id: Option<MessageId>,

    /// The message this one forwards, recorded as its origin; the caller must be able to read
    /// it.
    pub forwarded_from_message_id: Option<MessageId>,
}

/// An encrypted message, sent from the session's device under a client-chosen id.
#[derive(Debug, Clone)]
pub struct SendEncryptedMessageCmd {
    pub caller: crate::e2ee::command::MlsCaller,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    /// The `MLSMessage` bytes as unpadded base64url.
    pub envelope: String,
    pub thread_root_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
}

/// A new envelope for an encrypted message, carrying its complete new content.
#[derive(Debug, Clone)]
pub struct EditEncryptedMessageCmd {
    pub caller: crate::e2ee::command::MlsCaller,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub envelope: String,
}

#[derive(Debug, Clone, Copy)]
pub struct GetMessageQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, Copy)]
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
pub struct EditMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,

    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ForwardMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub source_conversation_id: ConversationId,
    pub target_conversation_id: ConversationId,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct DeleteMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,

    pub hard: bool,
}

/// One conversation to read in [`ListMessageChangesQuery`], with the version the client last
/// synced to (`0` for "from the beginning").
#[derive(Debug, Clone, Copy)]
pub struct MessageChangeCursor {
    pub conversation_id: ConversationId,
    pub after_version: i64,
}

/// The message change feed: what changed in these conversations since each one's cursor.
#[derive(Debug, Clone)]
pub struct ListMessageChangesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversations: Vec<MessageChangeCursor>,
    /// Maximum changes returned per conversation.
    pub limit: u32,
}
