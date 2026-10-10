use async_graphql::*;
use rim_core::server_feature::model::ServerFeature;
use rim_services::Actor;
use rim_services::message_attachment::command::GetAttachmentQuery;
use rim_services::message_attachment::service::MessageAttachmentService;

use crate::auth_context::require_actor;
use crate::features::require_feature;
use crate::ids::parse_uuid_id;
use crate::types::Attachment;

#[derive(Default)]
pub(crate) struct AttachmentQueries;

#[Object]
impl AttachmentQueries {
    async fn attachment(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        attachment_id: ID,
    ) -> Result<Attachment> {
        require_feature(ctx, ServerFeature::Attachments)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&attachment_id)?;

        let attachment = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor),
                server_id,
                attachment_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(attachment.into())
    }
}
