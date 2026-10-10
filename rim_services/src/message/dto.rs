use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message::repository::MessageCursor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::message_attachment::dto::AttachmentDto;

#[derive(Debug, Clone)]
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

    /// The MLS envelope of an encrypted message; `None` for plaintext.
    pub encryption: Option<MessageEncryptionDto>,
}

/// An encrypted message's envelope as members see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEncryptionDto {
    pub incarnation: i32,
    pub epoch: i64,
    /// The sender's client device id.
    pub sender_device_id: uuid::Uuid,
    /// The `MLSMessage` bytes; `None` once deleted.
    pub ciphertext: Option<Vec<u8>>,
    pub franking_receipt: Vec<u8>,
    pub franking_key_id: i32,
    /// `sent_at` in microseconds since the Unix epoch, as covered by the receipt.
    pub sent_at_us: i64,
}

/// What a [`MessageChangeDto`] says happened to the message, derived from its state at its
/// newest version: deleted wins over edited, and a message whose text never changed is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageChangeKind {
    Created,
    Edited,
    Deleted,
}

/// One entry of the message change feed: the message as it is now, at its newest `version`.
#[derive(Debug, Clone)]
pub struct MessageChangeDto {
    pub kind: MessageChangeKind,
    pub version: i64,
    pub edited_at: Option<chrono::DateTime<chrono::Utc>>,
    pub message: MessageDto,
}

/// How one conversation of a change feed request was answered. Reported per conversation, so
/// one channel the caller lost, or one reset counter, does not stall the sync of the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationChangesStatus {
    /// The changes after the requested cursor.
    Ok,
    /// The caller cannot read the conversation (removed, never a member of a private one, or it
    /// does not exist in the workspace). No changes; the cursor is echoed.
    NoAccess,
    /// The requested cursor is ahead of the conversation's counter (the counters were reset:
    /// a migration re-run or a restored backup). The changes are read from the beginning; the
    /// client drops its cached copy of the conversation before applying them.
    Reset,
}

/// The changes of one conversation since the requested version, oldest first.
#[derive(Debug, Clone)]
pub struct ConversationMessageChangesDto {
    pub conversation_id: ConversationId,
    pub status: ConversationChangesStatus,
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
pub struct MessageSearchHitDto {
    pub message: MessageDto,
    /// `ts_rank` score against the query — informational, not the sort key. See
    /// `DefaultMessageService::search_messages` for why relevance is not used for pagination.
    pub rank: f32,
    /// `ts_headline` snippet with `<mark>...</mark>` highlights around matched terms.
    pub headline: Option<String>,
}

#[derive(Debug, Clone, Copy)]
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
