use async_graphql::*;
use rim_services::Actor;
use rim_services::moderation::command::{GetReportQuery, ListBansQuery, ListReportsQuery};
use rim_services::moderation::service::ModerationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    MessageReport, MessageReportCursorInput, MessageReportPage, MessageReportStatus, ServerBan,
};

#[derive(Default)]
pub(crate) struct ModerationQueries;

#[Object]
impl ModerationQueries {
    // ── Moderation ────────────────────────────────────────────────────

    /// Active bans of the workspace, newest first. Owner/Moderator only.
    async fn server_bans(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<ServerBan>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ModerationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let bans = svc
            .list_bans(ListBansQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(map_service_error)?;
        Ok(bans.into_iter().map(Into::into).collect())
    }

    /// Message reports of the workspace (all statuses when `status` is omitted), ordered
    /// `createdAt ASC, id ASC`. Owner/Moderator only; for the direct space, instance operators
    /// only. The message body is included only when the moderator can read the conversation.
    async fn message_reports(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        status: Option<MessageReportStatus>,
        dir: PageDir,
        cursor: Option<MessageReportCursorInput>,
        limit: i32,
    ) -> Result<MessageReportPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ModerationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_reports(ListReportsQuery {
                actor: Actor::new(actor),
                server_id,
                status: status.map(Into::into),
                limit: (limit.max(1) as u32).min(100),
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(MessageReportPage::from_page(page))
    }

    /// One report of the workspace. Owner/Moderator only; for the direct space, instance
    /// operators only. For a verified report of an encrypted message it includes
    /// `frankedContent`, and records that the moderator viewed it (`e2ee.report_viewed`).
    async fn message_report(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        report_id: ID,
    ) -> Result<MessageReport> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ModerationService>>()?;
        let report = svc
            .get_report(GetReportQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                report_id: parse_uuid_id(&report_id)?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(report.into())
    }
}
