use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;

#[derive(Default)]
pub(crate) struct OnCallQueries;

#[Object]
impl OnCallQueries {
    /// List a server's on-call schedules. Any workspace member may read.
    async fn on_call_schedules(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<crate::types::OnCallScheduleGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let schedules = svc
            .list_schedules(rim_services::oncall::command::ListOnCallSchedulesQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(schedules.into_iter().map(Into::into).collect())
    }

    /// Resolve who is on call for a schedule right now (or at `at`, if provided). Any workspace
    /// member may read.
    async fn who_is_on_call(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<crate::types::OnCallNowGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id =
            crate::ids::parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let now = svc
            .who_is_on_call(rim_services::oncall::command::WhoIsOnCallQuery {
                actor: Actor::new(actor),
                schedule_id,
                at,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(now.into())
    }

    /// Batch [`who_is_on_call`] across every on-call schedule on a server — avoids an N+1 of
    /// individual `whoIsOnCall` calls for a status-bar-style UI. Any workspace member may read.
    async fn server_on_call(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<crate::types::OnCallNowGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let results = svc
            .server_on_call(rim_services::oncall::command::ServerOnCallQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(results.into_iter().map(Into::into).collect())
    }

    /// Resolve on-call coverage for a schedule over `[from, to)`. Rejects spans over 90 days. Any
    /// workspace member may read.
    async fn on_call_shifts(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::types::OnCallShiftGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id =
            crate::ids::parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let shifts = svc
            .list_shifts(rim_services::oncall::command::ListOnCallShiftsQuery {
                actor: Actor::new(actor),
                schedule_id,
                from,
                to,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(shifts.into_iter().map(Into::into).collect())
    }
}
