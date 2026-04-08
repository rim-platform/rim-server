use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
};
use rim_services::message_attachment::dto::{AttachmentDto, AttachmentStatus, UploadTicketDto};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::storage::service::StorageService;
use rim_services::{ServiceError, ServiceResult};

use crate::AttachmentRepos;
use crate::util::{
    ensure_conversation_access, ensure_conversation_in_server, ensure_server_member,
};

#[derive(Clone)]
pub struct DefaultMessageAttachmentService {
    repos: AttachmentRepos,
    storage_service: Arc<dyn StorageService>,
}

impl DefaultMessageAttachmentService {
    const PENDING_STATUS: &'static str = "pending_upload";
    const UPLOADED_STATUS: &'static str = "uploaded";
    const READY_STATUS: &'static str = "ready";
    const FAILED_STATUS: &'static str = "failed";

    /// Attachments in `PendingUpload` or `Uploaded` status older than this
    /// threshold are considered stale and reported as `Failed`.
    const STALE_THRESHOLD_MINUTES: i64 = 15;

    fn sanitize_filename(name: &str) -> Option<String> {
        rim_core::util::sanitize_filename(name)
    }

    pub fn new(repos: AttachmentRepos, storage_service: Arc<dyn StorageService>) -> Self {
        Self {
            repos,
            storage_service,
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
        }
    }

    async fn ensure_actor_can_access_message(
        &self,
        server_id: rim_core::server::model::ServerId,
        actor_id: rim_core::user::model::UserId,
        message_id: rim_core::message::model::MessageId,
    ) -> ServiceResult<rim_core::message::model::Message> {
        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            message.conversation_id,
            server_id,
        )
        .await?;

        // Use ensure_conversation_access which correctly handles channels
        // (channels only require server membership, not conversation_member rows)
        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            actor_id,
        )
        .await?;

        Ok(message)
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
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let message = self
            .ensure_actor_can_access_message(server_id, actor_id, att.message_id)
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
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let Some(message_id) = cmd.message_id else {
            return Err(ServiceError::Validation(
                "message_id is required in v0".to_string(),
            ));
        };

        let message = self
            .ensure_actor_can_access_message(cmd.server_id, cmd.actor.user_id, message_id)
            .await?;

        if message.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        if message.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

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
        if cmd.size_bytes > i64::MAX as u64 {
            return Err(ServiceError::Validation(
                "attachment size is too large".to_string(),
            ));
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
        };
        let attachment = self
            .repos
            .message_attachment_repo
            .add_attachment(new_attachment)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.repos
            .message_repo
            .set_has_attachments(message_id, true)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(UploadTicketDto {
            attachment_id: attachment.id,
            upload_token,
            max_size_bytes: Some(cmd.size_bytes),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::minutes(15)),
        })
    }

    async fn finalize_attachment(
        &self,
        cmd: FinalizeAttachmentCmd,
    ) -> ServiceResult<AttachmentDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

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
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::attachment_dto(updated))
    }

    async fn get_attachment(&self, query: GetAttachmentQuery) -> ServiceResult<AttachmentDto> {
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
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let _message = self
            .ensure_actor_can_access_message(query.server_id, query.actor.user_id, att.message_id)
            .await?;

        Ok(Self::attachment_dto(att))
    }

    async fn delete_attachment(&self, cmd: DeleteAttachmentCmd) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let (attachment, message) = self
            .ensure_actor_can_manage_attachment(cmd.server_id, cmd.actor.user_id, cmd.attachment_id)
            .await?;

        // Delete the file from storage before removing the DB record.
        // Ignore NotFound errors — the file may have been deleted manually.
        if !attachment.storage_url.starts_with("pending://") {
            if let Err(e) = self
                .storage_service
                .delete_file(&attachment.storage_url)
                .await
            {
                if !matches!(e, ServiceError::NotFound) {
                    return Err(e);
                }
            }
        }

        self.repos
            .message_attachment_repo
            .delete_attachment(attachment.id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let remaining = self
            .repos
            .message_attachment_repo
            .get_attachments_for_message(message.id)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.repos
            .message_repo
            .set_has_attachments(message.id, !remaining.is_empty())
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Verify the attachment is pending with matching token
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

        let uploaded_metadata = json!({
            "status": Self::UPLOADED_STATUS,
            "storage_filename": cmd.uploaded_filename,
        });

        let updated = self
            .repos
            .message_attachment_repo
            .update_attachment(cmd.attachment_id, cmd.storage_key, Some(uploaded_metadata))
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::attachment_dto(updated))
    }

    async fn authorize_download(
        &self,
        query: rim_services::message_attachment::command::AuthorizeDownloadQuery,
    ) -> ServiceResult<AttachmentDto> {
        let att = self
            .repos
            .message_attachment_repo
            .get_by_id(query.attachment_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Look up the message to find the conversation
        let message = self
            .repos
            .message_repo
            .get_by_id(att.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Check conversation access using the shared helper
        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(message.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // For channels: server membership suffices
        // For DMs/groups: explicit conversation membership required
        if conversation.conversation_type == rim_core::conversation::model::ConversationType::Channel {
            ensure_server_member(
                self.repos.server_member_repo.as_ref(),
                conversation.server_id,
                query.actor_user_id,
            )
            .await?;
        } else {
            ensure_conversation_access(
                self.repos.conversation_member_repo.as_ref(),
                &conversation,
                query.actor_user_id,
            )
            .await?;
        }

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
            .await
            .map_err(rim_services::map_repo_error)?;

        let count = stale_ids.len() as u64;
        for id in stale_ids {
            let att = self
                .repos
                .message_attachment_repo
                .get_by_id(id)
                .await
                .map_err(rim_services::map_repo_error)?;

            let Some(att) = att else { continue };

            self.repos
                .message_attachment_repo
                .update_attachment(
                    att.id,
                    att.storage_url,
                    Some(json!({ "status": Self::FAILED_STATUS })),
                )
                .await
                .map_err(rim_services::map_repo_error)?;
        }

        Ok(count)
    }
}
