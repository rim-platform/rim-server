use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message::repository::MessageCursor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::message_attachment::dto::AttachmentDto;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageDto {
    pub id: MessageId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,

    pub author_id: UserId,

    pub text: Option<String>,
    pub has_attachments: bool,
    pub attachments: Vec<AttachmentDto>,

    /// Threading / replies:
    pub parent_message_id: Option<MessageId>,

    /// Forwarding: the original message this was forwarded from.
    pub forwarded_from_message_id: Option<MessageId>,

    /// For thread replies: the root message ID of the thread (= thread.first_message_id).
    pub thread_root_message_id: Option<MessageId>,

    pub is_deleted: bool,

    pub sent_at: chrono::DateTime<chrono::Utc>,
}

/// What a [`MessageChangeDto`] says happened to the message, derived from its state at its
/// newest version: deleted wins over edited, and a message whose text never changed is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MessageChangeKind {
    Created,
    Edited,
    Deleted,
}

/// One entry of the message change feed: the message as it is now, at its newest `version`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageChangeDto {
    pub kind: MessageChangeKind,
    pub version: i64,
    pub edited_at: Option<chrono::DateTime<chrono::Utc>>,
    pub message: MessageDto,
}

/// The changes of one conversation since the requested version, oldest first.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConversationMessageChangesDto {
    pub conversation_id: ConversationId,
    pub changes: Vec<MessageChangeDto>,
    /// The version to pass back next time: the last change's version, or the requested one when
    /// nothing changed.
    pub cursor: i64,
    /// More changes follow `cursor`; ask again right away.
    pub has_more: bool,
}

/// A single full-text search hit: the matched message plus its relevance rank and an optional
/// highlighted snippet. Wraps [`MessageDto`] rather than flattening its fields, so callers that
/// only need the message (e.g. the `chat.search_messages` command executor's author/channel
/// resolution helpers, which take `&[MessageDto]`) can trivially project `hit.message`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageSearchHitDto {
    pub message: MessageDto,
    /// `ts_rank` score against the query — informational, not the sort key. See
    /// `DefaultMessageService::search_messages` for why relevance is not used for pagination.
    pub rank: f32,
    /// `ts_headline` snippet with `<mark>...</mark>` highlights around matched terms.
    pub headline: Option<String>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageReferenceDto {
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub permalink_path: String,
    pub anchor: String,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessagePageCursor {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: MessageId,
}

impl From<MessageCursor> for MessagePageCursor {
    fn from(value: MessageCursor) -> Self {
        Self {
            sent_at: value.sent_at,
            message_id: value.message_id,
        }
    }
}

impl From<MessagePageCursor> for MessageCursor {
    fn from(value: MessagePageCursor) -> Self {
        Self {
            sent_at: value.sent_at,
            message_id: value.message_id,
        }
    }
}
