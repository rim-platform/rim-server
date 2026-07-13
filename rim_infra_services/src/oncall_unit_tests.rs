#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Duration, NaiveTime, TimeZone, Utc};

    use rim_core::oncall::model::{
        OnCallOverride, OnCallOverrideId, OnCallSchedule, OnCallScheduleId, RotationKind,
        RotationLayer, RotationLayerId, RotationParticipant, RotationParticipantId,
    };
    use rim_core::oncall::repository::{
        MockOnCallOverrideRepository, MockOnCallScheduleRepository, MockRotationLayerRepository,
        MockRotationParticipantRepository,
    };
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::oncall::command::{
        AddRotationLayerCmd, CreateOnCallOverrideCmd, CreateOnCallScheduleCmd,
        DeleteOnCallOverrideCmd, DeleteOnCallScheduleCmd, ListOnCallSchedulesQuery,
        ListOnCallShiftsQuery, ListRotationLayersQuery, RemoveRotationLayerCmd, ServerOnCallQuery,
        SetRotationParticipantsCmd, UpdateOnCallScheduleCmd, UpdateRotationLayerCmd,
        WhoIsOnCallQuery,
    };
    use rim_services::oncall::service::OnCallService;
    use rim_services::{Actor, ServiceError};

    use crate::oncall::{DefaultOnCallService, OnCallRepos};

    fn owner_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Owner,
            joined_at: Utc::now(),
        }
    }

    fn guest_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Guest,
            joined_at: Utc::now(),
        }
    }

    fn moderator_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Moderator,
            joined_at: Utc::now(),
        }
    }

    fn make_schedule(server_id: ServerId, created_by: UserId) -> OnCallSchedule {
        let now = Utc::now();
        OnCallSchedule {
            id: OnCallScheduleId::new(),
            server_id,
            name: "Primary".to_string(),
            timezone: "America/Chicago".to_string(),
            created_by,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_layer(schedule_id: OnCallScheduleId, kind: RotationKind) -> RotationLayer {
        RotationLayer {
            id: RotationLayerId::new(),
            schedule_id,
            name: "Primary rotation".to_string(),
            priority: 0,
            kind,
            anchor_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            created_at: Utc::now(),
        }
    }

    fn make_participant(
        layer_id: RotationLayerId,
        user_id: UserId,
        position: i32,
    ) -> RotationParticipant {
        RotationParticipant {
            id: RotationParticipantId::new(),
            layer_id,
            user_id,
            position,
        }
    }

    fn build_service(
        schedule_repo: MockOnCallScheduleRepository,
        layer_repo: MockRotationLayerRepository,
        participant_repo: MockRotationParticipantRepository,
        override_repo: MockOnCallOverrideRepository,
        server_member_repo: MockServerMemberRepository,
    ) -> DefaultOnCallService {
        DefaultOnCallService::new(OnCallRepos {
            on_call_schedule_repo: Arc::new(schedule_repo),
            rotation_layer_repo: Arc::new(layer_repo),
            rotation_participant_repo: Arc::new(participant_repo),
            on_call_override_repo: Arc::new(override_repo),
            server_member_repo: Arc::new(server_member_repo),
        })
    }

    fn empty_repos() -> (
        MockOnCallScheduleRepository,
        MockRotationLayerRepository,
        MockRotationParticipantRepository,
        MockOnCallOverrideRepository,
    ) {
        (
            MockOnCallScheduleRepository::new(),
            MockRotationLayerRepository::new(),
            MockRotationParticipantRepository::new(),
            MockOnCallOverrideRepository::new(),
        )
    }

    // ── create_schedule ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_schedule_denies_non_owner() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(guest_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .create_schedule(CreateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                timezone: "America/Chicago".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_schedule_rejects_invalid_timezone() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .create_schedule(CreateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                timezone: "Not/ATimezone".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_schedule_succeeds_for_owner_with_valid_timezone() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        schedule_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(OnCallSchedule {
                    id: OnCallScheduleId::new(),
                    server_id: new.server_id,
                    name: new.name,
                    timezone: new.timezone,
                    created_by: new.created_by,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let dto = svc
            .create_schedule(CreateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                timezone: "America/Chicago".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.name, "Primary");
        assert_eq!(dto.timezone, "America/Chicago");
    }

    /// C1a-tail consistency fix: schedule/policy/override editing is Owner **or** Moderator
    /// (iron-man's ruling, §10) — C1a originally shipped Owner-only because it predated the
    /// ruling. A Moderator must now be able to create a schedule, not just an Owner.
    #[tokio::test]
    async fn create_schedule_succeeds_for_moderator() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        schedule_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(OnCallSchedule {
                    id: OnCallScheduleId::new(),
                    server_id: new.server_id,
                    name: new.name,
                    timezone: new.timezone,
                    created_by: new.created_by,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(moderator_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let dto = svc
            .create_schedule(CreateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                timezone: "America/Chicago".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.name, "Primary");
    }

    // ── update_schedule / delete_schedule / list_schedules ───────────────────

    #[tokio::test]
    async fn update_schedule_not_found_returns_not_found() {
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();
        schedule_repo
            .expect_find_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            MockServerMemberRepository::new(),
        );

        let err = svc
            .update_schedule(UpdateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                name: Some("New name".to_string()),
                timezone: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn update_schedule_denies_non_owner() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let creator_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, creator_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(guest_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .update_schedule(UpdateOnCallScheduleCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                name: Some("New name".to_string()),
                timezone: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn delete_schedule_succeeds_for_owner() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });
        schedule_repo
            .expect_delete()
            .withf(move |id| *id == schedule_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        svc.delete_schedule(DeleteOnCallScheduleCmd {
            actor: Actor::new(actor_id),
            schedule_id,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_schedules_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .list_schedules(ListOnCallSchedulesQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_schedules_returns_schedules_for_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let s1 = make_schedule(server_id, actor_id);
        schedule_repo.expect_list_by_server().returning(move |_| {
            let s1 = s1.clone();
            Box::pin(async move { Ok(vec![s1]) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let schedules = svc
            .list_schedules(ListOnCallSchedulesQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap();
        assert_eq!(schedules.len(), 1);
    }

    // ── list_rotation_layers ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_rotation_layers_returns_layers_with_participants_for_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, override_repo) =
            empty_repos();

        let schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        let layer_id = layer.id;
        layer_repo.expect_list_by_schedule().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(vec![layer]) })
        });

        let participant_user = UserId::new();
        participant_repo.expect_list_by_layer().returning(move |_| {
            Box::pin(async move { Ok(vec![make_participant(layer_id, participant_user, 0)]) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let layers = svc
            .list_rotation_layers(ListRotationLayersQuery {
                actor: Actor::new(actor_id),
                schedule_id,
            })
            .await
            .unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].participants, vec![participant_user]);
    }

    #[tokio::test]
    async fn list_rotation_layers_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .list_rotation_layers(ListRotationLayersQuery {
                actor: Actor::new(actor_id),
                schedule_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_rotation_layers_returns_not_found_for_missing_schedule() {
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        schedule_repo
            .expect_find_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        // No expectations on server_member_repo — the not-found check must short-circuit
        // before any membership check is attempted.
        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            MockServerMemberRepository::new(),
        );

        let err = svc
            .list_rotation_layers(ListRotationLayersQuery {
                actor: Actor::new(actor_id),
                schedule_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    // ── add_rotation_layer / update / remove / set_participants ─────────────

    #[tokio::test]
    async fn add_rotation_layer_rejects_non_positive_custom_interval() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .add_rotation_layer(AddRotationLayerCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                name: "Layer".to_string(),
                priority: 0,
                kind: RotationKind::Custom {
                    interval_seconds: 0,
                },
                anchor_at: Utc::now(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// C1 review (resource bound): a positive but sub-5-minute interval is still rejected — the
    /// floor is `>= 300`, not merely `> 0`. Keeps `resolve_shifts`'s legitimate worst-case
    /// boundary count bounded (see `MIN_CUSTOM_INTERVAL_SECONDS`'s doc comment).
    #[tokio::test]
    async fn add_rotation_layer_rejects_interval_below_minimum_floor() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .add_rotation_layer(AddRotationLayerCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                name: "Layer".to_string(),
                priority: 0,
                kind: RotationKind::Custom {
                    interval_seconds: 299,
                },
                anchor_at: Utc::now(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn add_rotation_layer_succeeds_with_empty_participants() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, mut layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });
        layer_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(RotationLayer {
                    id: RotationLayerId::new(),
                    schedule_id: new.schedule_id,
                    name: new.name,
                    priority: new.priority,
                    kind: new.kind,
                    anchor_at: new.anchor_at,
                    created_at: Utc::now(),
                })
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let dto = svc
            .add_rotation_layer(AddRotationLayerCmd {
                actor: Actor::new(actor_id),
                schedule_id,
                name: "Layer".to_string(),
                priority: 5,
                kind: RotationKind::Daily {
                    handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                },
                anchor_at: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(dto.priority, 5);
        assert!(dto.participants.is_empty());
    }

    #[tokio::test]
    async fn update_rotation_layer_validates_new_kind_and_returns_updated_dto() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let layer_id = RotationLayerId::new();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, override_repo) =
            empty_repos();

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        layer_repo.expect_find_by_id().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(Some(layer)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });
        layer_repo
            .expect_update()
            .returning(move |id, name, priority, kind, anchor_at| {
                Box::pin(async move {
                    Ok(RotationLayer {
                        id,
                        schedule_id,
                        name: name.unwrap_or_else(|| "Primary rotation".to_string()),
                        priority: priority.unwrap_or(0),
                        kind: kind.unwrap_or(RotationKind::Custom {
                            interval_seconds: 3600,
                        }),
                        anchor_at: anchor_at.unwrap_or_else(Utc::now),
                        created_at: Utc::now(),
                    })
                })
            });
        participant_repo
            .expect_list_by_layer()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let dto = svc
            .update_rotation_layer(UpdateRotationLayerCmd {
                actor: Actor::new(actor_id),
                layer_id,
                name: Some("Renamed".to_string()),
                priority: Some(9),
                kind: None,
                anchor_at: None,
            })
            .await
            .unwrap();
        assert_eq!(dto.name, "Renamed");
        assert_eq!(dto.priority, 9);
    }

    #[tokio::test]
    async fn update_rotation_layer_rejects_non_positive_custom_interval() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let layer_id = RotationLayerId::new();
        let (mut schedule_repo, mut layer_repo, participant_repo, override_repo) = empty_repos();

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        layer_repo.expect_find_by_id().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(Some(layer)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .update_rotation_layer(UpdateRotationLayerCmd {
                actor: Actor::new(actor_id),
                layer_id,
                name: None,
                priority: None,
                kind: Some(RotationKind::Custom {
                    interval_seconds: -1,
                }),
                anchor_at: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn remove_rotation_layer_loads_owning_schedule_and_deletes() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let layer_id = RotationLayerId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, mut layer_repo, participant_repo, override_repo) = empty_repos();

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        layer_repo.expect_find_by_id().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(Some(layer)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });
        layer_repo
            .expect_delete()
            .withf(move |id| *id == layer_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        svc.remove_rotation_layer(RemoveRotationLayerCmd {
            actor: Actor::new(actor_id),
            layer_id,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_rotation_participants_replaces_and_returns_updated_dto() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let layer_id = RotationLayerId::new();
        let u1 = UserId::new();
        let u2 = UserId::new();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, override_repo) =
            empty_repos();

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        layer_repo.expect_find_by_id().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(Some(layer)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });
        participant_repo
            .expect_set_participants()
            .withf(move |lid, ids| *lid == layer_id && ids == [u1, u2])
            .returning(move |_, ids| {
                let rows: Vec<RotationParticipant> = ids
                    .iter()
                    .enumerate()
                    .map(|(i, uid)| make_participant(layer_id, *uid, i as i32))
                    .collect();
                Box::pin(async move { Ok(rows) })
            });
        participant_repo.expect_list_by_layer().returning(move |_| {
            Box::pin(async move {
                Ok(vec![
                    make_participant(layer_id, u1, 0),
                    make_participant(layer_id, u2, 1),
                ])
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        // Membership validation (C1a-tail): every participant must be a member of the
        // schedule's server.
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let dto = svc
            .set_rotation_participants(SetRotationParticipantsCmd {
                actor: Actor::new(actor_id),
                layer_id,
                user_ids: vec![u1, u2],
            })
            .await
            .unwrap();
        assert_eq!(dto.participants, vec![u1, u2]);
    }

    /// C1a-tail (iron-man's ruling): a cross-server user id must reject the whole call
    /// atomically — `set_participants` must never be reached.
    #[tokio::test]
    async fn set_rotation_participants_rejects_non_member_user_id() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let layer_id = RotationLayerId::new();
        let member_user = UserId::new();
        let non_member_user = UserId::new();
        let (mut schedule_repo, mut layer_repo, participant_repo, override_repo) = empty_repos();

        let layer = make_layer(
            schedule_id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        layer_repo.expect_find_by_id().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(Some(layer)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });
        // No `expect_set_participants()` — the membership rejection must happen before any
        // write, so a call here would be a test failure (mockall panics on an unexpected call).

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        server_member_repo
            .expect_is_member()
            .returning(move |_, uid| Ok(uid == member_user));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .set_rotation_participants(SetRotationParticipantsCmd {
                actor: Actor::new(actor_id),
                layer_id,
                user_ids: vec![member_user, non_member_user],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // ── create_override / delete_override ────────────────────────────────────

    #[tokio::test]
    async fn create_override_rejects_start_after_end() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        // Membership validation (C1a-tail) runs before the start/end check this test targets.
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let now = Utc::now();
        let err = svc
            .create_override(CreateOnCallOverrideCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                user_id: UserId::new(),
                start_at: now,
                end_at: now - Duration::hours(1),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// C1a-tail (iron-man's ruling): the target user must be a member of the schedule's server
    /// — otherwise a cross-server user could be put on-call.
    #[tokio::test]
    async fn create_override_rejects_non_member_user_id() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });
        // No `expect_create()` on the override repo — rejection must happen before any write.

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let now = Utc::now();
        let err = svc
            .create_override(CreateOnCallOverrideCmd {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                user_id: UserId::new(),
                start_at: now,
                end_at: now + Duration::hours(1),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn delete_override_loads_owning_schedule_and_deletes() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let override_id = OnCallOverrideId::new();
        let (mut schedule_repo, layer_repo, participant_repo, mut override_repo) = empty_repos();

        let over = OnCallOverride {
            id: override_id,
            schedule_id,
            user_id: UserId::new(),
            start_at: Utc::now(),
            end_at: Utc::now() + Duration::hours(1),
            created_by: actor_id,
        };
        override_repo.expect_find_by_id().returning(move |_| {
            let over = over.clone();
            Box::pin(async move { Ok(Some(over)) })
        });
        let existing_schedule = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing_schedule = existing_schedule.clone();
            Box::pin(async move { Ok(Some(existing_schedule)) })
        });
        override_repo
            .expect_delete()
            .withf(move |id| *id == override_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        svc.delete_override(DeleteOnCallOverrideCmd {
            actor: Actor::new(actor_id),
            override_id,
        })
        .await
        .unwrap();
    }

    // ── who_is_on_call ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn who_is_on_call_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .who_is_on_call(WhoIsOnCallQuery {
                actor: Actor::new(actor_id),
                schedule_id: OnCallScheduleId::new(),
                at: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    /// Integration-correctness test (mocked repos): the service's `who_is_on_call` must match
    /// what `rim_core::oncall::resolve::resolve_on_call` computes over the same inputs — this is
    /// the wiring the db-test (`who_is_on_call_matches_pure_resolver`) also re-verifies against
    /// real rows.
    #[tokio::test]
    async fn who_is_on_call_resolves_current_user_via_custom_layer() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let u0 = UserId::new();
        let u1 = UserId::new();
        let anchor = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, mut override_repo) =
            empty_repos();

        let schedule = OnCallSchedule {
            id: schedule_id,
            server_id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });

        let layer = RotationLayer {
            id: RotationLayerId::new(),
            schedule_id,
            name: "Primary".to_string(),
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            created_at: Utc::now(),
        };
        let layer_id = layer.id;
        layer_repo.expect_list_by_schedule().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(vec![layer]) })
        });
        participant_repo.expect_list_by_layer().returning(move |_| {
            Box::pin(async move {
                Ok(vec![
                    make_participant(layer_id, u0, 0),
                    make_participant(layer_id, u1, 1),
                ])
            })
        });
        override_repo
            .expect_list_by_schedule()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        // At the anchor: u0 is on call, shift ends in exactly 1 hour, u1 is next.
        let result = svc
            .who_is_on_call(WhoIsOnCallQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                at: Some(anchor),
            })
            .await
            .unwrap();
        assert_eq!(result.current_user_id, Some(u0));
        assert_eq!(result.shift_ends_at, Some(anchor + Duration::hours(1)));
        assert_eq!(result.next_user_id, Some(u1));
    }

    #[tokio::test]
    async fn who_is_on_call_prefers_active_override_over_layer() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let layer_user = UserId::new();
        let override_user = UserId::new();
        let anchor = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, mut override_repo) =
            empty_repos();

        let schedule = OnCallSchedule {
            id: schedule_id,
            server_id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });

        let layer = RotationLayer {
            id: RotationLayerId::new(),
            schedule_id,
            name: "Primary".to_string(),
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            created_at: Utc::now(),
        };
        let layer_id = layer.id;
        layer_repo.expect_list_by_schedule().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(vec![layer]) })
        });
        participant_repo.expect_list_by_layer().returning(move |_| {
            Box::pin(async move { Ok(vec![make_participant(layer_id, layer_user, 0)]) })
        });

        let override_end = anchor + Duration::minutes(30);
        let over = OnCallOverride {
            id: OnCallOverrideId::new(),
            schedule_id,
            user_id: override_user,
            start_at: anchor - Duration::minutes(10),
            end_at: override_end,
            created_by: actor_id,
        };
        override_repo.expect_list_by_schedule().returning(move |_| {
            let over = over.clone();
            Box::pin(async move { Ok(vec![over]) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let result = svc
            .who_is_on_call(WhoIsOnCallQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                at: Some(anchor),
            })
            .await
            .unwrap();
        assert_eq!(result.current_user_id, Some(override_user));
        assert_eq!(result.shift_ends_at, Some(override_end));
        // After the override ends, the layer's participant (still shift 0 at that instant) takes over.
        assert_eq!(result.next_user_id, Some(layer_user));
    }

    #[tokio::test]
    async fn who_is_on_call_returns_none_for_empty_schedule() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, mut layer_repo, participant_repo, mut override_repo) =
            empty_repos();

        let schedule = OnCallSchedule {
            id: schedule_id,
            server_id,
            name: "Empty".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });
        layer_repo
            .expect_list_by_schedule()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        override_repo
            .expect_list_by_schedule()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let result = svc
            .who_is_on_call(WhoIsOnCallQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                at: None,
            })
            .await
            .unwrap();
        assert_eq!(result.current_user_id, None);
        assert_eq!(result.shift_ends_at, None);
        assert_eq!(result.next_user_id, None);
    }

    // ── server_on_call ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn server_on_call_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let (schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let err = svc
            .server_on_call(ServerOnCallQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn server_on_call_batches_across_every_schedule() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let u1 = UserId::new();
        let u2 = UserId::new();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, mut override_repo) =
            empty_repos();

        let schedule1 = OnCallSchedule {
            id: OnCallScheduleId::new(),
            server_id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let schedule2 = OnCallSchedule {
            id: OnCallScheduleId::new(),
            server_id,
            name: "Secondary".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let (s1, s2) = (schedule1.clone(), schedule2.clone());
        schedule_repo.expect_list_by_server().returning(move |_| {
            let (s1, s2) = (s1.clone(), s2.clone());
            Box::pin(async move { Ok(vec![s1, s2]) })
        });

        let layer1 = make_layer(
            schedule1.id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        let layer1_id = layer1.id;
        let layer2 = make_layer(
            schedule2.id,
            RotationKind::Custom {
                interval_seconds: 3600,
            },
        );
        let layer2_id = layer2.id;
        layer_repo
            .expect_list_by_schedule()
            .withf(move |id| *id == schedule1.id)
            .returning(move |_| {
                let layer1 = layer1.clone();
                Box::pin(async move { Ok(vec![layer1]) })
            });
        layer_repo
            .expect_list_by_schedule()
            .withf(move |id| *id == schedule2.id)
            .returning(move |_| {
                let layer2 = layer2.clone();
                Box::pin(async move { Ok(vec![layer2]) })
            });
        participant_repo
            .expect_list_by_layer()
            .withf(move |id| *id == layer1_id)
            .returning(move |_| {
                Box::pin(async move { Ok(vec![make_participant(layer1_id, u1, 0)]) })
            });
        participant_repo
            .expect_list_by_layer()
            .withf(move |id| *id == layer2_id)
            .returning(move |_| {
                Box::pin(async move { Ok(vec![make_participant(layer2_id, u2, 0)]) })
            });
        override_repo
            .expect_list_by_schedule()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let results = svc
            .server_on_call(ServerOnCallQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].schedule_id, schedule1.id);
        assert_eq!(results[0].current_user_id, Some(u1));
        assert_eq!(results[1].schedule_id, schedule2.id);
        assert_eq!(results[1].current_user_id, Some(u2));
    }

    // ── list_shifts ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_shifts_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let now = Utc::now();
        let err = svc
            .list_shifts(ListOnCallShiftsQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                from: now,
                to: now + Duration::hours(1),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_shifts_rejects_inverted_range() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let now = Utc::now();
        let err = svc
            .list_shifts(ListOnCallShiftsQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                from: now,
                to: now - Duration::hours(1),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn list_shifts_rejects_span_over_90_days() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let (mut schedule_repo, layer_repo, participant_repo, override_repo) = empty_repos();

        let existing = make_schedule(server_id, actor_id);
        schedule_repo.expect_find_by_id().returning(move |_| {
            let existing = existing.clone();
            Box::pin(async move { Ok(Some(existing)) })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let now = Utc::now();
        let err = svc
            .list_shifts(ListOnCallShiftsQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                from: now,
                to: now + Duration::days(91),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn list_shifts_returns_shifts_matching_the_pure_resolver() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let schedule_id = OnCallScheduleId::new();
        let u0 = UserId::new();
        let u1 = UserId::new();
        let anchor = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (mut schedule_repo, mut layer_repo, mut participant_repo, mut override_repo) =
            empty_repos();

        let schedule = OnCallSchedule {
            id: schedule_id,
            server_id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: actor_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        schedule_repo.expect_find_by_id().returning(move |_| {
            let schedule = schedule.clone();
            Box::pin(async move { Ok(Some(schedule)) })
        });

        let layer = RotationLayer {
            id: RotationLayerId::new(),
            schedule_id,
            name: "Primary".to_string(),
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
            created_at: Utc::now(),
        };
        let layer_id = layer.id;
        layer_repo.expect_list_by_schedule().returning(move |_| {
            let layer = layer.clone();
            Box::pin(async move { Ok(vec![layer]) })
        });
        participant_repo.expect_list_by_layer().returning(move |_| {
            Box::pin(async move {
                Ok(vec![
                    make_participant(layer_id, u0, 0),
                    make_participant(layer_id, u1, 1),
                ])
            })
        });
        override_repo
            .expect_list_by_schedule()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_service(
            schedule_repo,
            layer_repo,
            participant_repo,
            override_repo,
            server_member_repo,
        );

        let shifts = svc
            .list_shifts(ListOnCallShiftsQuery {
                actor: Actor::new(actor_id),
                schedule_id,
                from: anchor,
                to: anchor + Duration::hours(2),
            })
            .await
            .unwrap();
        assert_eq!(shifts.len(), 2);
        assert_eq!(shifts[0].user_id, Some(u0));
        assert_eq!(shifts[0].start_at, anchor);
        assert_eq!(shifts[0].end_at, anchor + Duration::hours(1));
        assert_eq!(shifts[1].user_id, Some(u1));
        assert_eq!(shifts[1].start_at, anchor + Duration::hours(1));
        assert_eq!(shifts[1].end_at, anchor + Duration::hours(2));
    }
}
