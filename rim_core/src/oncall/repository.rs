use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::oncall::model::{
    NewOnCallOverride, NewOnCallSchedule, NewRotationLayer, OnCallOverride, OnCallOverrideId,
    OnCallSchedule, OnCallScheduleId, RotationKind, RotationLayer, RotationLayerId,
    RotationParticipant,
};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait OnCallScheduleRepository: Send + Sync {
    async fn create(&self, new: NewOnCallSchedule) -> RepoResult<OnCallSchedule>;

    async fn find_by_id(&self, id: OnCallScheduleId) -> RepoResult<Option<OnCallSchedule>>;

    /// List all schedules on a server, ordered by `created_at ASC, id ASC`.
    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<OnCallSchedule>>;

    /// Updates `name`/`timezone`. `None` for a field leaves it unchanged.
    async fn update(
        &self,
        id: OnCallScheduleId,
        name: Option<String>,
        timezone: Option<String>,
    ) -> RepoResult<OnCallSchedule>;

    /// Deletes the schedule and (via FK cascade) its rotation layers, participants, and
    /// overrides.
    async fn delete(&self, id: OnCallScheduleId) -> RepoResult<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RotationLayerRepository: Send + Sync {
    async fn create(&self, new: NewRotationLayer) -> RepoResult<RotationLayer>;

    async fn find_by_id(&self, id: RotationLayerId) -> RepoResult<Option<RotationLayer>>;

    /// List all layers on a schedule, ordered by `priority DESC, created_at ASC`.
    async fn list_by_schedule(
        &self,
        schedule_id: OnCallScheduleId,
    ) -> RepoResult<Vec<RotationLayer>>;

    /// Updates `name`/`priority`/`kind`/`anchor_at`. `None` for a field leaves it unchanged.
    #[allow(clippy::too_many_arguments)]
    async fn update(
        &self,
        id: RotationLayerId,
        name: Option<String>,
        priority: Option<i32>,
        kind: Option<RotationKind>,
        anchor_at: Option<DateTime<Utc>>,
    ) -> RepoResult<RotationLayer>;

    /// Deletes the layer and (via FK cascade) its participants.
    async fn delete(&self, id: RotationLayerId) -> RepoResult<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RotationParticipantRepository: Send + Sync {
    /// List a layer's participants, ordered by `position ASC`.
    async fn list_by_layer(
        &self,
        layer_id: RotationLayerId,
    ) -> RepoResult<Vec<RotationParticipant>>;

    /// Replace the full ordered participant list for a layer (delete-then-insert). `user_ids[0]`
    /// becomes `position = 0`, and so on. An empty slice clears the layer's participants.
    async fn set_participants(
        &self,
        layer_id: RotationLayerId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<RotationParticipant>>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait OnCallOverrideRepository: Send + Sync {
    async fn create(&self, new: NewOnCallOverride) -> RepoResult<OnCallOverride>;

    async fn find_by_id(&self, id: OnCallOverrideId) -> RepoResult<Option<OnCallOverride>>;

    /// List all overrides on a schedule, ordered by `start_at ASC`. The service layer feeds the
    /// full list into [`crate::oncall::resolve::ScheduleSnapshot`] — override counts per schedule
    /// are expected to stay small in practice, and a full list (rather than a time-windowed one)
    /// is what lets the resolver correctly answer "who's next" across an override boundary.
    async fn list_by_schedule(
        &self,
        schedule_id: OnCallScheduleId,
    ) -> RepoResult<Vec<OnCallOverride>>;

    async fn delete(&self, id: OnCallOverrideId) -> RepoResult<()>;
}
