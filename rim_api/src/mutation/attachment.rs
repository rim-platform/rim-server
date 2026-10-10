use async_graphql::*;
use rim_core::server_feature::model::ServerFeature;
use rim_services::Actor;
use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd,
};
use rim_services::message_attachment::service::MessageAttachmentService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::features::require_feature;
use crate::ids::parse_uuid_id;
use crate::types::{Attachment, UploadTicket};

#[derive(Default)]
pub(crate) struct AttachmentMutations;

#[Object]
impl AttachmentMutations {
    async fn create_attachment(
        &self,
        ctx: &Context<'_>,
        input: CreateAttachmentInput,
    ) -> Result<UploadTicket> {
        require_feature(ctx, ServerFeature::Attachments)?;
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = input
            .message_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::message::model::MessageId>)
            .transpose()?;

        let ticket = svc
            .create_attachment(CreateAttachmentCmd {
                blob_sha256: input.blob_sha256,
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                filename: input.filename,
                content_type: input.content_type,
                size_bytes: input.size_bytes,
            })
            .await
            .map_err(map_service_error)?;

        Ok(ticket.into())
    }

    async fn finalize_attachment(
        &self,
        ctx: &Context<'_>,
        input: FinalizeAttachmentInput,
    ) -> Result<Attachment> {
        require_feature(ctx, ServerFeature::Attachments)?;
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&input.attachment_id)?;

        let attachment = svc
            .finalize_attachment(FinalizeAttachmentCmd {
                actor: Actor::new(actor),
                server_id,
                attachment_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(attachment.into())
    }

    async fn delete_attachment(
        &self,
        ctx: &Context<'_>,
        input: DeleteAttachmentInput,
    ) -> Result<bool> {
        require_feature(ctx, ServerFeature::Attachments)?;
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&input.attachment_id)?;

        svc.delete_attachment(DeleteAttachmentCmd {
            actor: Actor::new(actor),
            server_id,
            attachment_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct CreateAttachmentInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: Option<ID>,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
    /// Required in an encrypted conversation: unpadded base64url SHA-256 of the blob.
    pub blob_sha256: Option<String>,
}

#[derive(InputObject)]
pub struct FinalizeAttachmentInput {
    pub server_id: ID,
    pub attachment_id: ID,
}

#[derive(InputObject)]
pub struct DeleteAttachmentInput {
    pub server_id: ID,
    pub attachment_id: ID,
}
