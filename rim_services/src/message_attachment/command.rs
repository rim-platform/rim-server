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
/// attachment is in pending state, and updates the storage key.
#[derive(Debug, Clone)]
pub struct CompleteUploadCmd {
    pub actor_user_id: rim_core::user::model::UserId,
    pub attachment_id: MessageAttachmentId,
    pub upload_token: String,
    pub storage_key: String,
    pub uploaded_filename: String,
}

/// Authorize a download: verify the requesting user has access to the
/// attachment's conversation, and return the attachment details.
#[derive(Debug, Clone, Copy)]
pub struct AuthorizeDownloadQuery {
    pub actor_user_id: rim_core::user::model::UserId,
    pub attachment_id: MessageAttachmentId,
}
