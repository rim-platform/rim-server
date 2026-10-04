use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;

use rim_core::oncall::model::{
    NewOnCallOverride, NewOnCallSchedule, NewRotationLayer, OnCallOverrideId, RotationKind,
    RotationLayerId,
};
use rim_core::oncall::repository::{
    OnCallOverrideRepository, OnCallScheduleRepository, RotationLayerRepository,
    RotationParticipantRepository,
};
use rim_core::oncall::resolve::{
    ResolvedLayer, ScheduleSnapshot, next_boundary, resolve_on_call, resolve_shifts,
};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::ServiceError;
use rim_services::ServiceResult;
use rim_services::oncall::command::{
    AddRotationLayerCmd, CreateOnCallOverrideCmd, CreateOnCallScheduleCmd, DeleteOnCallOverrideCmd,
    DeleteOnCallScheduleCmd, ListOnCallSchedulesQuery, ListOnCallShiftsQuery,
    ListRotationLayersQuery, RemoveRotationLayerCmd, ServerOnCallQuery, SetRotationParticipantsCmd,
    UpdateOnCallScheduleCmd, UpdateRotationLayerCmd, WhoIsOnCallQuery,
};
use rim_services::oncall::dto::{
    OnCallNowDto, OnCallOverrideDto, OnCallScheduleDto, OnCallShiftDto, RotationLayerDto,
    rotation_layer_dto,
};
use rim_services::oncall::service::OnCallService;

use crate::util::ensure_server_owner_or_moderator;

#[derive(Clone)]
pub struct OnCallRepos {
    pub on_call_schedule_repo: Arc<dyn OnCallScheduleRepository>,
    pub rotation_layer_repo: Arc<dyn RotationLayerRepository>,
    pub rotation_participant_repo: Arc<dyn RotationParticipantRepository>,
    pub on_call_override_repo: Arc<dyn OnCallOverrideRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

pub struct DefaultOnCallService {
    repos: OnCallRepos,
}

impl DefaultOnCallService {
    pub fn new(repos: OnCallRepos) -> Self {
        Self { repos }
    }

    /// Validates an IANA timezone name, matching the constraint documented on
    /// `OnCallSchedule::timezone`.
    fn validate_timezone(timezone: &str) -> ServiceResult<()> {
        Tz::from_str(timezone)
            .map_err(|_| ServiceError::Validation(format!("invalid IANA timezone: {timezone}")))?;
        Ok(())
    }

    /// Rejects a `Custom` rotation whose `interval_seconds` is non-positive or below
    /// `MIN_CUSTOM_INTERVAL_SECONDS` (C1 review, resource bound): sub-5-minute on-call rotations
    /// are operationally nonsensical (nobody hands off on-call every few seconds) and, combined
    /// with `MAX_SHIFT_QUERY_SPAN`, this floor keeps `resolve_shifts`'s worst-case legitimate
    /// boundary count bounded (90d / 300s = 25,920) — see `MAX_CUSTOM_BOUNDARIES` in
    /// `rim_core::oncall::resolve` for the independent hard backstop that protects `resolve_shifts`
    /// itself regardless of whether this floor is bypassed (e.g. by a schedule persisted before
    /// this floor existed).
    fn validate_kind(kind: &RotationKind) -> ServiceResult<()> {
        if let RotationKind::Custom { interval_seconds } = kind
            && *interval_seconds < MIN_CUSTOM_INTERVAL_SECONDS
        {
            return Err(ServiceError::Validation(format!(
                "custom rotation interval_seconds must be >= {MIN_CUSTOM_INTERVAL_SECONDS}"
            )));
        }
        Ok(())
    }

    /// Loads the schedule owning `layer_id` and gates on server ownership. Used by every
    /// layer-scoped mutation, since layers don't carry `server_id` directly.
    async fn ensure_owner_via_layer(
        &self,
        actor: rim_core::user::model::UserId,
        layer_id: RotationLayerId,
    ) -> ServiceResult<rim_core::oncall::model::RotationLayer> {
        let layer = self
            .repos
            .rotation_layer_repo
            .find_by_id(layer_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(layer.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            actor,
        )
        .await?;
        Ok(layer)
    }

    /// Loads the schedule owning `override_id` and gates on server ownership.
    async fn ensure_owner_via_override(
        &self,
        actor: rim_core::user::model::UserId,
        override_id: OnCallOverrideId,
    ) -> ServiceResult<rim_core::oncall::model::OnCallOverride> {
        let over = self
            .repos
            .on_call_override_repo
            .find_by_id(override_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(over.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            actor,
        )
        .await?;
        Ok(over)
    }

    /// Validates every `user_id` is a member of `server_id`, rejecting the whole call
    /// atomically with `Validation` on the first non-member found (iron-man's ruling on
    /// `setRotationParticipants`/`createOnCallOverride`: a cross-server user could otherwise be
    /// put on-call). Write-time check only — if a member later leaves the server, the schedule
    /// still degrades gracefully at page time per §5; this does not self-heal that case.
    async fn ensure_all_members(
        &self,
        server_id: rim_core::server::model::ServerId,
        user_ids: &[rim_core::user::model::UserId],
    ) -> ServiceResult<()> {
        for &user_id in user_ids {
            let is_member = self
                .repos
                .server_member_repo
                .is_member(server_id, user_id)
                .await?;
            if !is_member {
                return Err(ServiceError::Validation(format!(
                    "user {user_id} is not a member of this schedule's server"
                )));
            }
        }
        Ok(())
    }

    async fn build_layer_dto(
        &self,
        layer: rim_core::oncall::model::RotationLayer,
    ) -> ServiceResult<RotationLayerDto> {
        let participants = self
            .repos
            .rotation_participant_repo
            .list_by_layer(layer.id)
            .await?
            .into_iter()
            .map(|p| p.user_id)
            .collect();
        Ok(rotation_layer_dto(layer, participants))
    }

    /// Loads everything needed to build a [`ScheduleSnapshot`] for `schedule`: its parsed
    /// timezone, every rotation layer with its participants (owned, since `ScheduleSnapshot`
    /// borrows), and every override. Shared by `who_is_on_call`/`server_on_call` (via
    /// [`Self::resolve_now`]) and `list_shifts`.
    async fn load_schedule(
        &self,
        schedule: &rim_core::oncall::model::OnCallSchedule,
    ) -> ServiceResult<LoadedSchedule> {
        load_schedule_from_repos(&self.repos, schedule).await
    }

    /// Resolve [`OnCallNowDto`] for `schedule` at `now` — the shared body of `who_is_on_call` and
    /// `server_on_call`, which differ only in how the schedule is looked up and gated.
    async fn resolve_now(
        &self,
        schedule: &rim_core::oncall::model::OnCallSchedule,
        now: chrono::DateTime<Utc>,
    ) -> ServiceResult<OnCallNowDto> {
        let loaded = self.load_schedule(schedule).await?;
        let resolved_layers = loaded.resolved_layers();
        let snapshot = ScheduleSnapshot {
            timezone: loaded.tz,
            layers: &resolved_layers,
            overrides: &loaded.overrides,
        };

        let current_user_id = resolve_on_call(&snapshot, now);

        // Determine the current assignment's end and who follows it.
        let (shift_ends_at, next_user_id) = if current_user_id.is_none() {
            (None, None)
        } else if let Some(active_override) = loaded
            .overrides
            .iter()
            .filter(|o| o.start_at <= now && now < o.end_at)
            .max_by_key(|o| uuid::Uuid::from(o.id))
        {
            // An override is currently in effect: it ends at `end_at`, after which whatever the
            // resolver says at that instant takes over (which may be another override or a layer).
            let next = resolve_on_call(&snapshot, active_override.end_at);
            (Some(active_override.end_at), next)
        } else {
            // A layer is currently on call: find its next handoff boundary.
            let mut layers_by_priority: Vec<&ResolvedLayer> = resolved_layers.iter().collect();
            layers_by_priority.sort_by_key(|l| std::cmp::Reverse(l.priority));
            let current_layer = layers_by_priority
                .into_iter()
                .find(|l| !l.participants.is_empty());
            match current_layer {
                Some(layer) => {
                    let boundary = next_boundary(layer, loaded.tz, now);
                    let next = boundary.and_then(|b| resolve_on_call(&snapshot, b));
                    (boundary, next)
                }
                None => (None, None),
            }
        };

        Ok(OnCallNowDto {
            schedule_id: schedule.id,
            current_user_id,
            shift_ends_at,
            next_user_id,
        })
    }
}

/// Free-function form of [`DefaultOnCallService::load_schedule`], taking `repos` directly
/// rather than `&self` so it can be shared with escalation step firing
/// (`DefaultIncidentService::resolve_step_target`, a background context with no `OnCallService`
/// actor/permission gate to satisfy — see `docs/oncall.md` "Escalation wiring").
async fn load_schedule_from_repos(
    repos: &OnCallRepos,
    schedule: &rim_core::oncall::model::OnCallSchedule,
) -> ServiceResult<LoadedSchedule> {
    let tz = Tz::from_str(&schedule.timezone).map_err(|e| {
        ServiceError::internal(anyhow::anyhow!(
            "schedule {} has invalid stored timezone {:?}: {e}",
            schedule.id,
            schedule.timezone
        ))
    })?;

    let layers = repos
        .rotation_layer_repo
        .list_by_schedule(schedule.id)
        .await?;
    let mut layer_participants = Vec::with_capacity(layers.len());
    for layer in &layers {
        let participants = repos
            .rotation_participant_repo
            .list_by_layer(layer.id)
            .await?
            .into_iter()
            .map(|p| p.user_id)
            .collect::<Vec<_>>();
        layer_participants.push(participants);
    }

    let overrides = repos
        .on_call_override_repo
        .list_by_schedule(schedule.id)
        .await?;

    Ok(LoadedSchedule {
        tz,
        layers,
        layer_participants,
        overrides,
    })
}

/// Resolves "who is on call now" for a schedule by id, with no actor/permission gate — used by
/// escalation step firing to resolve an `EscalationTarget::OnCall(schedule_id)` (§5). Returns
/// `Ok(None)` (rather than an error) both when the schedule no longer exists and when it has no
/// current coverage, so a step target that's gone stale after authoring degrades gracefully
/// (log + skip actions, still advance the chain) instead of aborting escalation.
pub(crate) async fn resolve_on_call_now_for_schedule(
    repos: &OnCallRepos,
    schedule_id: rim_core::oncall::model::OnCallScheduleId,
    now: chrono::DateTime<Utc>,
) -> ServiceResult<Option<rim_core::user::model::UserId>> {
    let Some(schedule) = repos.on_call_schedule_repo.find_by_id(schedule_id).await? else {
        return Ok(None);
    };
    let loaded = load_schedule_from_repos(repos, &schedule).await?;
    let resolved_layers = loaded.resolved_layers();
    let snapshot = ScheduleSnapshot {
        timezone: loaded.tz,
        layers: &resolved_layers,
        overrides: &loaded.overrides,
    };
    Ok(resolve_on_call(&snapshot, now))
}

/// Owned data backing a [`ScheduleSnapshot`] — `ScheduleSnapshot` borrows, so this struct is what
/// keeps the data alive across the snapshot's lifetime at each call site.
struct LoadedSchedule {
    tz: Tz,
    layers: Vec<rim_core::oncall::model::RotationLayer>,
    layer_participants: Vec<Vec<rim_core::user::model::UserId>>,
    overrides: Vec<rim_core::oncall::model::OnCallOverride>,
}

impl LoadedSchedule {
    fn resolved_layers(&self) -> Vec<ResolvedLayer<'_>> {
        self.layers
            .iter()
            .zip(self.layer_participants.iter())
            .map(|(layer, participants)| ResolvedLayer {
                priority: layer.priority,
                kind: layer.kind,
                anchor_at: layer.anchor_at,
                participants: participants.as_slice(),
            })
            .collect()
    }
}

/// `onCallShifts`'s maximum queryable span — keeps `resolve_shifts`'s boundary walk bounded (a
/// `Custom` layer with a short `interval_seconds` over an unbounded range would otherwise
/// enumerate an unbounded number of shifts).
const MAX_SHIFT_QUERY_SPAN: chrono::Duration = chrono::Duration::days(90);

/// The minimum `interval_seconds` a `Custom` rotation layer may be created/updated with (C1
/// review, resource bound) — see `validate_kind`'s doc comment for the rationale.
const MIN_CUSTOM_INTERVAL_SECONDS: i64 = 300;

#[async_trait]
impl OnCallService for DefaultOnCallService {
    async fn create_schedule(
        &self,
        cmd: CreateOnCallScheduleCmd,
    ) -> ServiceResult<OnCallScheduleDto> {
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        Self::validate_timezone(&cmd.timezone)?;

        let schedule = self
            .repos
            .on_call_schedule_repo
            .create(NewOnCallSchedule {
                server_id: cmd.server_id,
                name: cmd.name,
                timezone: cmd.timezone,
                created_by: cmd.actor.user_id,
            })
            .await?;
        Ok(schedule.into())
    }

    async fn update_schedule(
        &self,
        cmd: UpdateOnCallScheduleCmd,
    ) -> ServiceResult<OnCallScheduleDto> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(cmd.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            cmd.actor.user_id,
        )
        .await?;
        if let Some(timezone) = &cmd.timezone {
            Self::validate_timezone(timezone)?;
        }

        let updated = self
            .repos
            .on_call_schedule_repo
            .update(cmd.schedule_id, cmd.name, cmd.timezone)
            .await?;
        Ok(updated.into())
    }

    async fn delete_schedule(&self, cmd: DeleteOnCallScheduleCmd) -> ServiceResult<()> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(cmd.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            cmd.actor.user_id,
        )
        .await?;
        self.repos
            .on_call_schedule_repo
            .delete(cmd.schedule_id)
            .await?;
        Ok(())
    }

    async fn list_schedules(
        &self,
        query: ListOnCallSchedulesQuery,
    ) -> ServiceResult<Vec<OnCallScheduleDto>> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        let schedules = self
            .repos
            .on_call_schedule_repo
            .list_by_server(query.server_id)
            .await?;
        Ok(schedules.into_iter().map(Into::into).collect())
    }

    async fn list_rotation_layers(
        &self,
        query: ListRotationLayersQuery,
    ) -> ServiceResult<Vec<RotationLayerDto>> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(query.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            query.actor.user_id,
        )
        .await?;

        let layers = self
            .repos
            .rotation_layer_repo
            .list_by_schedule(query.schedule_id)
            .await?;
        let mut dtos = Vec::with_capacity(layers.len());
        for layer in layers {
            dtos.push(self.build_layer_dto(layer).await?);
        }
        Ok(dtos)
    }

    async fn add_rotation_layer(
        &self,
        cmd: AddRotationLayerCmd,
    ) -> ServiceResult<RotationLayerDto> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(cmd.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            cmd.actor.user_id,
        )
        .await?;
        Self::validate_kind(&cmd.kind)?;

        let layer = self
            .repos
            .rotation_layer_repo
            .create(NewRotationLayer {
                schedule_id: cmd.schedule_id,
                name: cmd.name,
                priority: cmd.priority,
                kind: cmd.kind,
                anchor_at: cmd.anchor_at,
            })
            .await?;
        // A freshly created layer has no participants yet.
        Ok(rotation_layer_dto(layer, Vec::new()))
    }

    async fn update_rotation_layer(
        &self,
        cmd: UpdateRotationLayerCmd,
    ) -> ServiceResult<RotationLayerDto> {
        self.ensure_owner_via_layer(cmd.actor.user_id, cmd.layer_id)
            .await?;
        if let Some(kind) = &cmd.kind {
            Self::validate_kind(kind)?;
        }

        let updated = self
            .repos
            .rotation_layer_repo
            .update(
                cmd.layer_id,
                cmd.name,
                cmd.priority,
                cmd.kind,
                cmd.anchor_at,
            )
            .await?;
        self.build_layer_dto(updated).await
    }

    async fn remove_rotation_layer(&self, cmd: RemoveRotationLayerCmd) -> ServiceResult<()> {
        self.ensure_owner_via_layer(cmd.actor.user_id, cmd.layer_id)
            .await?;
        self.repos.rotation_layer_repo.delete(cmd.layer_id).await?;
        Ok(())
    }

    async fn set_rotation_participants(
        &self,
        cmd: SetRotationParticipantsCmd,
    ) -> ServiceResult<RotationLayerDto> {
        let layer = self
            .ensure_owner_via_layer(cmd.actor.user_id, cmd.layer_id)
            .await?;
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(layer.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.ensure_all_members(schedule.server_id, &cmd.user_ids)
            .await?;
        self.repos
            .rotation_participant_repo
            .set_participants(cmd.layer_id, &cmd.user_ids)
            .await?;
        self.build_layer_dto(layer).await
    }

    async fn create_override(
        &self,
        cmd: CreateOnCallOverrideCmd,
    ) -> ServiceResult<OnCallOverrideDto> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(cmd.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            cmd.actor.user_id,
        )
        .await?;
        self.ensure_all_members(schedule.server_id, std::slice::from_ref(&cmd.user_id))
            .await?;
        if cmd.start_at >= cmd.end_at {
            return Err(ServiceError::Validation(
                "override start_at must be before end_at".to_string(),
            ));
        }

        let over = self
            .repos
            .on_call_override_repo
            .create(NewOnCallOverride {
                schedule_id: cmd.schedule_id,
                user_id: cmd.user_id,
                start_at: cmd.start_at,
                end_at: cmd.end_at,
                created_by: cmd.actor.user_id,
            })
            .await?;
        Ok(over.into())
    }

    async fn delete_override(&self, cmd: DeleteOnCallOverrideCmd) -> ServiceResult<()> {
        self.ensure_owner_via_override(cmd.actor.user_id, cmd.override_id)
            .await?;
        self.repos
            .on_call_override_repo
            .delete(cmd.override_id)
            .await?;
        Ok(())
    }

    async fn who_is_on_call(&self, query: WhoIsOnCallQuery) -> ServiceResult<OnCallNowDto> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(query.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            query.actor.user_id,
        )
        .await?;

        let now = query.at.unwrap_or_else(Utc::now);
        self.resolve_now(&schedule, now).await
    }

    async fn server_on_call(&self, query: ServerOnCallQuery) -> ServiceResult<Vec<OnCallNowDto>> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let schedules = self
            .repos
            .on_call_schedule_repo
            .list_by_server(query.server_id)
            .await?;

        let now = Utc::now();
        let mut results = Vec::with_capacity(schedules.len());
        for schedule in &schedules {
            results.push(self.resolve_now(schedule, now).await?);
        }
        Ok(results)
    }

    async fn list_shifts(
        &self,
        query: ListOnCallShiftsQuery,
    ) -> ServiceResult<Vec<OnCallShiftDto>> {
        let schedule = self
            .repos
            .on_call_schedule_repo
            .find_by_id(query.schedule_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            schedule.server_id,
            query.actor.user_id,
        )
        .await?;

        if query.from >= query.to {
            return Err(ServiceError::Validation(
                "from must be before to".to_string(),
            ));
        }
        // Keeps the boundary walk in `resolve_shifts` bounded — see its doc comment.
        if query.to - query.from > MAX_SHIFT_QUERY_SPAN {
            return Err(ServiceError::Validation(
                "onCallShifts span must not exceed 90 days".to_string(),
            ));
        }

        let loaded = self.load_schedule(&schedule).await?;
        let resolved_layers = loaded.resolved_layers();
        let snapshot = ScheduleSnapshot {
            timezone: loaded.tz,
            layers: &resolved_layers,
            overrides: &loaded.overrides,
        };

        let shifts = resolve_shifts(&snapshot, query.from, query.to);
        Ok(shifts.into_iter().map(Into::into).collect())
    }
}
