use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
};
use rim_services::message_attachment::dto::{AttachmentDto, AttachmentStatus, UploadTicketDto};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_attachment::{MAX_ATTACHMENT_BYTES, UPLOAD_TICKET_TTL_MINUTES};
use rim_services::storage::service::StorageService;
use rim_services::{ServiceError, ServiceResult};

use crate::AttachmentRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_conversation_writable,
    ensure_server_member,
};

/// The only file name and media type an encrypted conversation's attachment is created with:
/// the real ones travel encrypted in the message.
const ENCRYPTED_FILENAME: &str = "encrypted";
const ENCRYPTED_CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Clone)]
pub struct DefaultMessageAttachmentService {
    repos: AttachmentRepos,
    storage_service: Arc<dyn StorageService>,
    /// Serializes the quota check and the insert in `create_attachment`, so concurrent
    /// requests can't all pass the check before any of them is counted. One lock for every
    /// workspace is enough: the critical section is two short queries, and the server runs
    /// as a single process (presence and typing state are in-memory too).
    quota_lock: Arc<tokio::sync::Mutex<()>>,
}

impl DefaultMessageAttachmentService {
    const PENDING_STATUS: &'static str = "pending_upload";
    const UPLOADED_STATUS: &'static str = "uploaded";
    const READY_STATUS: &'static str = "ready";
    const FAILED_STATUS: &'static str = "failed";

    /// Attachments in `PendingUpload` or `Uploaded` status older than this
    /// threshold are considered stale and reported as `Failed`.
    const STALE_THRESHOLD_MINUTES: i64 = UPLOAD_TICKET_TTL_MINUTES;

    fn sanitize_filename(name: &str) -> Option<String> {
        rim_core::util::sanitize_filename(name)
    }

    pub fn new(repos: AttachmentRepos, storage_service: Arc<dyn StorageService>) -> Self {
        Self {
            repos,
            storage_service,
            quota_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn metadata_status(metadata: Option<&serde_json::Value>) -> AttachmentStatus {
        let status = metadata
            .and_then(|value| value.get("status"))
            .and_then(|value| value.as_str());

        match status {
            Some(Self::PENDING_STATUS) => AttachmentStatus::PendingUpload,
            Some(Self::UPLOADED_STATUS) => AttachmentStatus::Uploaded,
            Some(Self::FAILED_STATUS) => AttachmentStatus::Failed,
            _ => AttachmentStatus::Ready,
        }
    }

    /// Extract the creation timestamp from a UUID v7 attachment ID.
    fn created_at_from_id(
        id: rim_core::message_attachment::model::MessageAttachmentId,
    ) -> chrono::DateTime<chrono::Utc> {
        let uuid_val: uuid::Uuid = id.into();
        let (seconds, nanos) = uuid_val.get_timestamp().unwrap().to_unix();
        chrono::DateTime::from_timestamp(seconds as i64, nanos).unwrap_or_default()
    }

    /// Returns `true` when the attachment is in a non-terminal upload state
    /// and was created more than [`Self::STALE_THRESHOLD_MINUTES`] ago.
    fn is_stale(status: &AttachmentStatus, created_at: chrono::DateTime<chrono::Utc>) -> bool {
        matches!(
            status,
            AttachmentStatus::PendingUpload | AttachmentStatus::Uploaded
        ) && chrono::Utc::now()
            .signed_duration_since(created_at)
            .num_minutes()
            >= Self::STALE_THRESHOLD_MINUTES
    }

    pub(crate) fn attachment_dto(
        att: rim_core::message_attachment::model::MessageAttachment,
    ) -> AttachmentDto {
        let mut status = Self::metadata_status(att.metadata.as_ref());
        let created_at = Self::created_at_from_id(att.id);
        if Self::is_stale(&status, created_at) {
            status = AttachmentStatus::Failed;
        }
        let storage_key = if att.storage_url.starts_with("pending://") {
            None
        } else {
            Some(att.storage_url)
        };
        AttachmentDto {
            id: att.id,
            status,
            filename: att.file_name,
            content_type: att.mime_type,
            size_bytes: att.file_size.max(0) as u64,
            storage_key,
            created_at,
            blob_sha256: att.blob_sha256,
        }
    }

    fn access_repos(&self) -> ConversationAccessRepos<'_> {
        ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        )
    }

    /// Best-effort delete of a stored file that no attachment will reference.
    async fn discard_upload(&self, key: &str) {
        if let Err(e) = self.storage_service.delete_file(key).await {
            tracing::warn!(error = %e, key = %key, "failed to delete orphaned upload");
        }
    }

    /// Verifies the actor is a member of the conversation (via the full 3-step
    /// gate: server membership → conversation in server → conversation access),
    /// then returns the loaded message and its conversation. With `write`, an archived
    /// conversation is rejected with `ConversationArchived` as well.
    async fn ensure_actor_can_access_message(
        &self,
        server_id: rim_core::server::model::ServerId,
        actor_id: rim_core::user::model::UserId,
        message_id: rim_core::message::model::MessageId,
        write: bool,
    ) -> ServiceResult<(
        rim_core::message::model::Message,
        rim_core::conversation::model::Conversation,
    )> {
        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let conversation = if write {
            ensure_conversation_writable(
                self.access_repos(),
                actor_id,
                server_id,
                message.conversation_id,
            )
            .await?
        } else {
            ensure_conversation_member_access(
                self.access_repos(),
                actor_id,
                server_id,
                message.conversation_id,
            )
            .await?
        };

        Ok((message, conversation))
    }

    async fn ensure_actor_can_manage_attachment(
        &self,
        server_id: rim_core::server::model::ServerId,
        actor_id: rim_core::user::model::UserId,
        attachment_id: rim_core::message_attachment::model::MessageAttachmentId,
    ) -> ServiceResult<(
        rim_core::message_attachment::model::MessageAttachment,
        rim_core::message::model::Message,
    )> {
        let att = self
            .repos
            .message_attachment_repo
            .get_by_id(attachment_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let (message, _) = self
            .ensure_actor_can_access_message(server_id, actor_id, att.message_id, true)
            .await?;

        if message.sent_by != actor_id {
            return Err(ServiceError::PermissionDenied);
        }

        Ok((att, message))
    }
}

#[async_trait]
impl MessageAttachmentService for DefaultMessageAttachmentService {
    async fn create_attachment(&self, cmd: CreateAttachmentCmd) -> ServiceResult<UploadTicketDto> {
        let Some(message_id) = cmd.message_id else {
            return Err(ServiceError::Validation(
                "message_id is required in v0".to_string(),
            ));
        };

        let (message, conversation) = self
            .ensure_actor_can_access_message(cmd.server_id, cmd.actor.user_id, message_id, true)
            .await?;

        if message.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        if message.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        let blob_sha256 = match (&cmd.blob_sha256, conversation.is_encrypted()) {
            (Some(hash), true) => {
                if cmd.filename != ENCRYPTED_FILENAME || cmd.content_type != ENCRYPTED_CONTENT_TYPE
                {
                    return Err(ServiceError::Validation(format!(
                        "an encrypted conversation's attachment is named `{ENCRYPTED_FILENAME}` \
                         with type `{ENCRYPTED_CONTENT_TYPE}`"
                    )));
                }
                Some(crate::e2ee::decode_fixed::<32>("blobSha256", hash)?.to_vec())
            }
            (None, true) => {
                return Err(ServiceError::Validation(
                    "blobSha256 is required in an encrypted conversation".to_string(),
                ));
            }
            (Some(_), false) => {
                return Err(ServiceError::Validation(
                    "blobSha256 is only for encrypted conversations".to_string(),
                ));
            }
            (None, false) => None,
        };

        let sanitized_filename = Self::sanitize_filename(&cmd.filename).ok_or_else(|| {
            ServiceError::Validation("attachment filename is invalid".to_string())
        })?;

        if cmd.content_type.trim().is_empty() {
            return Err(ServiceError::Validation(
                "attachment content_type must be non-empty".to_string(),
            ));
        }
        if cmd.size_bytes == 0 {
            return Err(ServiceError::Validation(
                "attachment size must be greater than zero".to_string(),
            ));
        }
        if cmd.size_bytes > MAX_ATTACHMENT_BYTES {
            return Err(ServiceError::Validation(format!(
                "attachment is too large: {} bytes (max {MAX_ATTACHMENT_BYTES} bytes)",
                cmd.size_bytes
            )));
        }

        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();
        let upload_token = uuid::Uuid::new_v4().to_string();
        let storage_filename = format!("{attachment_id}_{sanitized_filename}");
        let metadata = Some(json!({
            "status": Self::PENDING_STATUS,
            "storage_filename": storage_filename,
        }));
        let new_attachment = rim_core::message_attachment::model::NewMessageAttachment {
            id: attachment_id,
            message_id,
            storage_url: format!("pending://{upload_token}"),
            file_name: sanitized_filename,
            file_size: cmd.size_bytes as i64,
            mime_type: cmd.content_type.trim().to_owned(),
            metadata,
            blob_sha256,
        };
        // The declared size is reserved against the workspace quota now, while the upload is
        // still pending; `complete_upload` refuses any file larger than what was declared.
        let attachment = {
            let _quota_guard = self.quota_lock.lock().await;
            let usage = self.storage_service.get_quota_usage(cmd.server_id).await?;
            if let Some(max) = usage.max_bytes
                && usage.used_bytes.saturating_add(cmd.size_bytes) > max
            {
                return Err(ServiceError::Validation(format!(
                    "storage quota exceeded: {} of {} bytes used",
                    usage.used_bytes, max
                )));
            }
            self.repos
                .message_attachment_repo
                .add_attachment(new_attachment)
                .await?
        };

        self.repos
            .message_repo
            .set_has_attachments(message_id, true)
            .await?;

        Ok(UploadTicketDto {
            attachment_id: attachment.id,
            upload_token,
            max_size_bytes: Some(cmd.size_bytes),
            expires_at: Some(
                chrono::Utc::now() + chrono::Duration::minutes(UPLOAD_TICKET_TTL_MINUTES),
            ),
        })
    }

    async fn finalize_attachment(
        &self,
        cmd: FinalizeAttachmentCmd,
    ) -> ServiceResult<AttachmentDto> {
        let (attachment, _message) = self
            .ensure_actor_can_manage_attachment(cmd.server_id, cmd.actor.user_id, cmd.attachment_id)
            .await?;

        // Verify the attachment has been uploaded before finalizing.
        let current_status = Self::metadata_status(attachment.metadata.as_ref());
        if current_status != AttachmentStatus::Uploaded {
            return Err(ServiceError::Validation(
                "attachment must be uploaded before finalizing".to_string(),
            ));
        }

        let updated = self
            .repos
            .message_attachment_repo
            .update_attachment(
                attachment.id,
                attachment.storage_url.clone(),
                Some(json!({ "status": Self::READY_STATUS })),
            )
            .await?;

        Ok(Self::attachment_dto(updated))
    }

    async fn get_attachment(&self, query: GetAttachmentQuery) -> ServiceResult<AttachmentDto> {
        // Preflight: reject non-server-members before probing attachment existence.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let att = self
            .repos
            .message_attachment_repo
            .get_by_id(query.attachment_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.ensure_actor_can_access_message(
            query.server_id,
            query.actor.user_id,
            att.message_id,
            false,
        )
        .await?;

        Ok(Self::attachment_dto(att))
    }

    async fn delete_attachment(&self, cmd: DeleteAttachmentCmd) -> ServiceResult<()> {
        let (attachment, message) = self
            .ensure_actor_can_manage_attachment(cmd.server_id, cmd.actor.user_id, cmd.attachment_id)
            .await?;

        // Delete the file from storage before removing the DB record.
        // Ignore NotFound errors — the file may have been deleted manually.
        if !attachment.storage_url.starts_with("pending://")
            && let Err(e) = self
                .storage_service
                .delete_file(&attachment.storage_url)
                .await
            && !matches!(e, ServiceError::NotFound)
        {
            return Err(e);
        }

        self.repos
            .message_attachment_repo
            .delete_attachment(attachment.id)
            .await?;

        let remaining = self
            .repos
            .message_attachment_repo
            .get_attachments_for_message(message.id)
            .await?;

        self.repos
            .message_repo
            .set_has_attachments(message.id, !remaining.is_empty())
            .await?;

        Ok(())
    }

    async fn complete_upload(
        &self,
        cmd: rim_services::message_attachment::command::CompleteUploadCmd,
    ) -> ServiceResult<AttachmentDto> {
        let attachment = self
            .repos
            .message_attachment_repo
            .get_by_id(cmd.attachment_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        // Verify the attachment is pending with matching token before any auth work,
        // so token probing leaks no information about attachment ownership.
        let expected_storage_url = format!("pending://{}", cmd.upload_token);
        if attachment.storage_url != expected_storage_url {
            return Err(ServiceError::Validation(
                "upload token already used or invalid".to_string(),
            ));
        }

        let current_status = Self::metadata_status(attachment.metadata.as_ref());
        if current_status != AttachmentStatus::PendingUpload {
            return Err(ServiceError::Validation(
                "attachment is not pending upload".to_string(),
            ));
        }

        // Once the ticket lapses its quota reservation is released, so it can't be filled.
        let reserved_since =
            chrono::Utc::now() - chrono::Duration::minutes(UPLOAD_TICKET_TTL_MINUTES);
        if Self::created_at_from_id(attachment.id) < reserved_since {
            return Err(ServiceError::Validation(
                "upload ticket expired".to_string(),
            ));
        }

        // Derive server_id from message → conversation, then enforce full
        // 3-step access gate (server membership + conv-in-server + conv access).
        let message = self
            .repos
            .message_repo
            .get_by_id(attachment.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(message.conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_conversation_writable(
            self.access_repos(),
            cmd.actor_user_id,
            conversation.server_id,
            conversation.id,
        )
        .await?;

        // Only the message author may upload files on behalf of their message.
        if message.sent_by != cmd.actor_user_id {
            return Err(ServiceError::PermissionDenied);
        }

        let uploaded_size = cmd.data.len() as u64;
        if uploaded_size == 0 {
            return Err(ServiceError::Validation(
                "uploaded file is empty".to_string(),
            ));
        }
        // An encrypted blob is opaque: exactly the declared size and hash, nothing sniffed.
        let is_octet_stream = cmd
            .content_type
            .trim()
            .eq_ignore_ascii_case(ENCRYPTED_CONTENT_TYPE);
        match &attachment.blob_sha256 {
            Some(expected) => {
                use sha2::Digest;
                if !is_octet_stream {
                    return Err(ServiceError::Validation(format!(
                        "an encrypted attachment is uploaded as {ENCRYPTED_CONTENT_TYPE}"
                    )));
                }
                if i64::try_from(uploaded_size).ok() != Some(attachment.file_size) {
                    return Err(ServiceError::Validation(
                        "the uploaded blob's size differs from the declared size".to_string(),
                    ));
                }
                if sha2::Sha256::digest(&cmd.data).as_slice() != expected.as_slice() {
                    return Err(ServiceError::Validation(
                        "the uploaded blob does not match blobSha256".to_string(),
                    ));
                }
            }
            None if is_octet_stream => {
                return Err(ServiceError::Validation(
                    "file type not allowed".to_string(),
                ));
            }
            None => {}
        }
        if uploaded_size > u64::try_from(attachment.file_size).unwrap_or(0) {
            return Err(ServiceError::Validation(format!(
                "uploaded file is {uploaded_size} bytes, larger than the {} bytes declared",
                attachment.file_size
            )));
        }

        // Every attempt gets its own key, so a request that loses the completion race below
        // can't overwrite the winner's file; it deletes its own instead.
        let key_filename =
            Self::sanitize_filename(&cmd.uploaded_filename).unwrap_or_else(|| "file".to_string());
        let storage_key = format!(
            "uploads/{}/{}_{}",
            cmd.actor_user_id,
            uuid::Uuid::now_v7(),
            key_filename
        );
        let stored = self
            .storage_service
            .upload_file(&storage_key, &cmd.data, &cmd.content_type)
            .await?;

        let uploaded_metadata = json!({
            "status": Self::UPLOADED_STATUS,
            "storage_filename": cmd.uploaded_filename,
        });

        let completed = self
            .repos
            .message_attachment_repo
            .try_complete_upload(
                cmd.attachment_id,
                expected_storage_url,
                // Re-read the clock: the ticket may have lapsed while the file was written.
                chrono::Utc::now() - chrono::Duration::minutes(UPLOAD_TICKET_TTL_MINUTES),
                stored.key.clone(),
                stored.size_bytes as i64,
                Some(uploaded_metadata),
            )
            .await;

        match completed {
            Ok(Some(updated)) => Ok(Self::attachment_dto(updated)),
            Ok(None) => {
                self.discard_upload(&stored.key).await;
                Err(ServiceError::Validation(
                    "upload token already used or invalid".to_string(),
                ))
            }
            Err(e) => {
                self.discard_upload(&stored.key).await;
                Err(e.into())
            }
        }
    }

    async fn authorize_download(
        &self,
        query: rim_services::message_attachment::command::AuthorizeDownloadQuery,
    ) -> ServiceResult<AttachmentDto> {
        let att = self
            .repos
            .message_attachment_repo
            .get_by_id(query.attachment_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let message = self
            .repos
            .message_repo
            .get_by_id(att.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(message.conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_conversation_member_access(
            self.access_repos(),
            query.actor_user_id,
            conversation.server_id,
            conversation.id,
        )
        .await?;

        Ok(Self::attachment_dto(att))
    }

    async fn cleanup_stale_attachments(&self, older_than_minutes: i64) -> ServiceResult<u64> {
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(older_than_minutes);
        let stale_ids = self
            .repos
            .message_attachment_repo
            .find_ids_with_status_before(
                vec![
                    Self::PENDING_STATUS.to_owned(),
                    Self::UPLOADED_STATUS.to_owned(),
                ],
                cutoff,
            )
            .await?;

        let count = stale_ids.len() as u64;
        for id in stale_ids {
            let att = self.repos.message_attachment_repo.get_by_id(id).await?;

            let Some(att) = att else { continue };

            self.repos
                .message_attachment_repo
                .update_attachment(
                    att.id,
                    att.storage_url,
                    Some(json!({ "status": Self::FAILED_STATUS })),
                )
                .await?;
        }

        Ok(count)
    }
}
