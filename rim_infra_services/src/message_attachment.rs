use async_trait::async_trait;
use serde_json::json;

use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
};
use rim_services::message_attachment::dto::{AttachmentDto, AttachmentStatus, UploadTicketDto};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::{ServiceError, ServiceResult};

use crate::AttachmentRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[cfg(test)]
use crate::test_support::attachment::{
    AttachmentReposPorts, ConversationMemberRepoPortAdapter, ConversationRepoPortAdapter,
    MessageAttachmentRepoPortAdapter, MessageRepoPortAdapter, ServerMemberRepoPortAdapter,
};

#[derive(Clone)]
pub struct DefaultMessageAttachmentService {
    repos: AttachmentRepos,
}

impl DefaultMessageAttachmentService {
    const PENDING_STATUS: &'static str = "pending_upload";
    const READY_STATUS: &'static str = "ready";

    pub fn new(repos: AttachmentRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: AttachmentReposPorts) -> Self {
        Self::new(AttachmentRepos {
            message_attachment_repo: std::sync::Arc::new(MessageAttachmentRepoPortAdapter(
                repos.message_attachment_repo,
            )),
            conversation_repo: std::sync::Arc::new(ConversationRepoPortAdapter(
                repos.conversation_repo,
            )),
            conversation_member_repo: std::sync::Arc::new(ConversationMemberRepoPortAdapter(
                repos.conversation_member_repo,
            )),
            message_repo: std::sync::Arc::new(MessageRepoPortAdapter(repos.message_repo)),
            server_member_repo: std::sync::Arc::new(ServerMemberRepoPortAdapter(
                repos.server_member_repo,
            )),
        })
    }

    pub(crate) fn metadata_status(metadata: Option<&serde_json::Value>) -> AttachmentStatus {
        let status = metadata
            .and_then(|value| value.get("status"))
            .and_then(|value| value.as_str());

        match status {
            Some(Self::PENDING_STATUS) => AttachmentStatus::PendingUpload,
            _ => AttachmentStatus::Ready,
        }
    }

    pub(crate) fn attachment_dto(
        att: rim_core::message_attachment::model::MessageAttachment,
    ) -> AttachmentDto {
        AttachmentDto {
            id: att.id,
            status: Self::metadata_status(att.metadata.as_ref()),
            filename: att.file_name,
            content_type: att.mime_type,
            size_bytes: att.file_size.max(0) as u64,
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

        let _conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            message.conversation_id,
            server_id,
        )
        .await?;

        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(message.conversation_id, actor_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }

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

        if cmd.filename.trim().is_empty() {
            return Err(ServiceError::Validation(
                "attachment filename must be non-empty".to_string(),
            ));
        }
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

        let upload_token = format!(
            "upload-{}",
            rim_core::message_attachment::model::MessageAttachmentId::new()
        );
        let metadata = Some(json!({
            "status": Self::PENDING_STATUS,
        }));
        let new_attachment = rim_core::message_attachment::model::NewMessageAttachment {
            message_id,
            storage_url: format!("pending://{upload_token}"),
            file_name: cmd.filename.trim().to_owned(),
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

        let updated = self
            .repos
            .message_attachment_repo
            .update_attachment(
                attachment.id,
                format!("attachment://{}", attachment.id),
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
}
