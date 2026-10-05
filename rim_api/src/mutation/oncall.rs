use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct OnCallMutations;

#[Object]
impl OnCallMutations {
    /// Create an on-call schedule. Owner-only.
    async fn create_on_call_schedule(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
        timezone: String,
    ) -> Result<crate::types::OnCallScheduleGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let dto = svc
            .create_schedule(rim_services::oncall::command::CreateOnCallScheduleCmd {
                actor: Actor::new(actor),
                server_id,
                name,
                timezone,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Update an on-call schedule's `name`/`timezone`. Owner-only.
    async fn update_on_call_schedule(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        name: Option<String>,
        timezone: Option<String>,
    ) -> Result<crate::types::OnCallScheduleGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let dto = svc
            .update_schedule(rim_services::oncall::command::UpdateOnCallScheduleCmd {
                actor: Actor::new(actor),
                schedule_id,
                name,
                timezone,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Delete an on-call schedule (cascades to its rotation layers/participants/overrides).
    /// Owner-only.
    async fn delete_on_call_schedule(&self, ctx: &Context<'_>, schedule_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        svc.delete_schedule(rim_services::oncall::command::DeleteOnCallScheduleCmd {
            actor: Actor::new(actor),
            schedule_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Add a rotation layer to a schedule. Owner-only.
    #[allow(clippy::too_many_arguments)]
    async fn add_rotation_layer(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        name: String,
        priority: i32,
        kind: crate::types::RotationKindInputGql,
        anchor_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;

        let dto = svc
            .add_rotation_layer(rim_services::oncall::command::AddRotationLayerCmd {
                actor: Actor::new(actor),
                schedule_id,
                name,
                priority,
                kind: kind.try_into()?,
                anchor_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Update a rotation layer's `name`/`priority`/`kind`/`anchorAt`. Owner-only (resolved via
    /// the layer's owning schedule).
    #[allow(clippy::too_many_arguments)]
    async fn update_rotation_layer(
        &self,
        ctx: &Context<'_>,
        layer_id: ID,
        name: Option<String>,
        priority: Option<i32>,
        kind: Option<crate::types::RotationKindInputGql>,
        anchor_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;

        let dto = svc
            .update_rotation_layer(rim_services::oncall::command::UpdateRotationLayerCmd {
                actor: Actor::new(actor),
                layer_id,
                name,
                priority,
                kind: kind.map(TryInto::try_into).transpose()?,
                anchor_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Remove a rotation layer (cascades to its participants). Owner-only.
    async fn remove_rotation_layer(&self, ctx: &Context<'_>, layer_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;

        svc.remove_rotation_layer(rim_services::oncall::command::RemoveRotationLayerCmd {
            actor: Actor::new(actor),
            layer_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Replace a rotation layer's full ordered participant list. `userIds[0]` becomes the first
    /// shift. Owner-only.
    async fn set_rotation_participants(
        &self,
        ctx: &Context<'_>,
        layer_id: ID,
        user_ids: Vec<ID>,
    ) -> Result<crate::types::RotationLayerGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let layer_id = parse_uuid_id::<rim_core::oncall::model::RotationLayerId>(&layer_id)?;
        let user_ids = user_ids
            .iter()
            .map(parse_uuid_id::<rim_core::user::model::UserId>)
            .collect::<Result<Vec<_>>>()?;

        let dto = svc
            .set_rotation_participants(rim_services::oncall::command::SetRotationParticipantsCmd {
                actor: Actor::new(actor),
                layer_id,
                user_ids,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Create a one-off on-call override for a schedule. Owner-only.
    async fn create_on_call_override(
        &self,
        ctx: &Context<'_>,
        schedule_id: ID,
        user_id: ID,
        start_at: chrono::DateTime<chrono::Utc>,
        end_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::types::OnCallOverrideGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let schedule_id = parse_uuid_id::<rim_core::oncall::model::OnCallScheduleId>(&schedule_id)?;
        let user_id = parse_uuid_id::<rim_core::user::model::UserId>(&user_id)?;

        let dto = svc
            .create_override(rim_services::oncall::command::CreateOnCallOverrideCmd {
                actor: Actor::new(actor),
                schedule_id,
                user_id,
                start_at,
                end_at,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }

    /// Delete an on-call override. Owner-only (resolved via the override's owning schedule).
    async fn delete_on_call_override(&self, ctx: &Context<'_>, override_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::oncall::service::OnCallService>>()?;
        let override_id = parse_uuid_id::<rim_core::oncall::model::OnCallOverrideId>(&override_id)?;

        svc.delete_override(rim_services::oncall::command::DeleteOnCallOverrideCmd {
            actor: Actor::new(actor),
            override_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}
