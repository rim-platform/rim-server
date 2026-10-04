use crate::Actor;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_attachment::model::MessageAttachmentId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateAttachmentCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub message_id: Option<MessageId>,

    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FinalizeAttachmentCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub attachment_id: MessageAttachmentId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteAttachmentCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub attachment_id: MessageAttachmentId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetAttachmentQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub attachment_id: MessageAttachmentId,
}

/// Completes a pending upload: validates the upload token, checks that the
/// attachment is in pending state and that `data` fits its declared size, then
/// stores `data` and records the storage key.
pub struct CompleteUploadCmd {
    pub actor_user_id: rim_core::user::model::UserId,
    pub attachment_id: MessageAttachmentId,
    pub upload_token: String,
    pub uploaded_filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for CompleteUploadCmd {
    // Hand-written so a debug log never dumps the file contents.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompleteUploadCmd")
            .field("actor_user_id", &self.actor_user_id)
            .field("attachment_id", &self.attachment_id)
            .field("uploaded_filename", &self.uploaded_filename)
            .field("content_type", &self.content_type)
            .field("data_len", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// Authorize a download: verify the requesting user has access to the
/// attachment's conversation, and return the attachment details.
#[derive(Debug, Clone, Copy)]
pub struct AuthorizeDownloadQuery {
    pub actor_user_id: rim_core::user::model::UserId,
    pub attachment_id: MessageAttachmentId,
}
