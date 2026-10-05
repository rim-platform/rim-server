use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    IncidentGql, IncidentListCursorInput, IncidentPage, IncidentSeverityGql, IncidentStatusGql,
    IncidentTimelineGql,
};

#[derive(Default)]
pub(crate) struct IncidentQueries;

#[Object]
impl IncidentQueries {
    // ── Incidents ──────────────────────────────────────────────────────

    async fn active_incident(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Option<IncidentGql>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let incident = svc
            .get_active_incident(rim_services::incident::command::GetActiveIncidentQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(incident.map(Into::into))
    }

    async fn incident_timeline(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<IncidentTimelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let timeline = svc
            .get_incident_timeline(rim_services::incident::command::GetIncidentTimelineQuery {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(timeline.into())
    }

    #[allow(clippy::too_many_arguments)]
    async fn list_incidents(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        status_filter: Option<Vec<IncidentStatusGql>>,
        severity_filter: Option<Vec<IncidentSeverityGql>>,
        dir: PageDir,
        cursor: Option<IncidentListCursorInput>,
        #[graphql(default = 20)] limit: i32,
    ) -> Result<IncidentPage> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_incidents(rim_services::incident::command::ListIncidentsQuery {
                actor: Actor::new(actor),
                server_id,
                status_filter: status_filter
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>()),
                severity_filter: severity_filter
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>()),
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(IncidentPage::from_page(page))
    }
}
