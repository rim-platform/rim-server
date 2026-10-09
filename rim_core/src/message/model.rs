use crate::conversation::model::ConversationId;
use crate::key_identity::model::DeviceKeyId;
use crate::message_attachment::model::NewMessageAttachment;
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(MessageId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub thread_id: Option<MessageThreadId>,
    pub parent_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub sent_at: DateTime<Utc>,
    pub sent_by: UserId,
    pub text: Option<String>,
    pub is_deleted: bool,
    pub has_attachments: bool,
    /// The MLS envelope of an encrypted message, whose `text` is always `None`.
    pub encryption: Option<MessageEncryption>,
}

/// The MLS envelope of an encrypted message row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEncryption {
    pub incarnation: i32,
    /// The epoch the application message was sent in.
    pub epoch: i64,
    pub sender_device_key_id: Option<DeviceKeyId>,
    /// The `MLSMessage` bytes; `None` once the message is deleted.
    pub ciphertext: Option<Vec<u8>>,
    /// The sender's HMAC commitment to the content, from the authenticated data.
    pub franking_commitment: Vec<u8>,
    /// The server's HMAC over the commitment and the message's ids and send time.
    pub franking_receipt: Vec<u8>,
    /// The server franking key the receipt was made with.
    pub franking_key_id: i32,
}

impl Message {
    pub fn is_timeline_message(&self) -> bool {
        self.thread_id.is_none()
    }

    pub fn is_thread_reply(&self) -> bool {
        self.thread_id.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub conversation_id: ConversationId,
    pub text: Option<String>,
    pub sent_by: UserId,
    pub thread_id: Option<MessageThreadId>,
    pub parent_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub attachments: Vec<NewMessageAttachment>,
}

impl NewMessage {
    pub fn is_timeline_message(&self) -> bool {
        self.thread_id.is_none()
    }

    pub fn is_thread_reply(&self) -> bool {
        self.thread_id.is_some()
    }
}
