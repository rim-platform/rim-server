use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::oncall::command::{
    AddRotationLayerCmd, CreateOnCallOverrideCmd, CreateOnCallScheduleCmd, DeleteOnCallOverrideCmd,
    DeleteOnCallScheduleCmd, ListOnCallSchedulesQuery, ListOnCallShiftsQuery,
    ListRotationLayersQuery, RemoveRotationLayerCmd, ServerOnCallQuery, SetRotationParticipantsCmd,
    UpdateOnCallScheduleCmd, UpdateRotationLayerCmd, WhoIsOnCallQuery,
};
use crate::oncall::dto::{
    OnCallNowDto, OnCallOverrideDto, OnCallScheduleDto, OnCallShiftDto, RotationLayerDto,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait OnCallService: Send + Sync {
    async fn create_schedule(
        &self,
        cmd: CreateOnCallScheduleCmd,
    ) -> ServiceResult<OnCallScheduleDto>;

    async fn update_schedule(
        &self,
        cmd: UpdateOnCallScheduleCmd,
    ) -> ServiceResult<OnCallScheduleDto>;

    async fn delete_schedule(&self, cmd: DeleteOnCallScheduleCmd) -> ServiceResult<()>;

    async fn list_schedules(
        &self,
        query: ListOnCallSchedulesQuery,
    ) -> ServiceResult<Vec<OnCallScheduleDto>>;

    async fn add_rotation_layer(&self, cmd: AddRotationLayerCmd)
    -> ServiceResult<RotationLayerDto>;

    /// `OnCallSchedule.rotationLayers` (§10) — a schedule's existing layers, each with its
    /// participants attached, ordered `priority DESC, created_at ASC` (mirrors
    /// `RotationLayerRepository::list_by_schedule`'s own ordering). Any server member may read —
    /// unlike every mutation above, this is not Owner/Moderator-gated.
    async fn list_rotation_layers(
        &self,
        query: ListRotationLayersQuery,
    ) -> ServiceResult<Vec<RotationLayerDto>>;

    async fn update_rotation_layer(
        &self,
        cmd: UpdateRotationLayerCmd,
    ) -> ServiceResult<RotationLayerDto>;

    async fn remove_rotation_layer(&self, cmd: RemoveRotationLayerCmd) -> ServiceResult<()>;

    /// Replace a layer's full ordered participant list. Returns the layer with the updated
    /// participant list attached.
    async fn set_rotation_participants(
        &self,
        cmd: SetRotationParticipantsCmd,
    ) -> ServiceResult<RotationLayerDto>;

    async fn create_override(
        &self,
        cmd: CreateOnCallOverrideCmd,
    ) -> ServiceResult<OnCallOverrideDto>;

    async fn delete_override(&self, cmd: DeleteOnCallOverrideCmd) -> ServiceResult<()>;

    /// Resolve who is on call for a schedule at [`WhoIsOnCallQuery::at`] (or now, if `None`),
    /// via [`rim_core::oncall::resolve::resolve_on_call`].
    async fn who_is_on_call(&self, query: WhoIsOnCallQuery) -> ServiceResult<OnCallNowDto>;

    /// Batch [`Self::who_is_on_call`] (at "now") across every schedule on a server.
    async fn server_on_call(&self, query: ServerOnCallQuery) -> ServiceResult<Vec<OnCallNowDto>>;

    /// Resolve on-call coverage over `[from, to)` via
    /// [`rim_core::oncall::resolve::resolve_shifts`]. Rejects spans over 90 days with
    /// `ServiceError::Validation`.
    async fn list_shifts(&self, query: ListOnCallShiftsQuery)
    -> ServiceResult<Vec<OnCallShiftDto>>;
}
