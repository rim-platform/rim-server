#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};
    use mockall::predicate::eq;

    use rim_core::conversation::model::ConversationId;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::escalation::repository::{
        MockConversationEscalationPolicyRepository, MockEscalationPolicyRepository,
        MockEscalationRunRepository, MockEscalationStepRepository,
    };
    use rim_core::incident::model::{Incident, IncidentId, IncidentSeverity, IncidentStatus};
    use rim_core::incident::repository::{
        IncidentListCursor, MockIncidentMessageTagRepository, MockIncidentPhaseRepository,
        MockIncidentRepository,
    };
    use rim_core::oncall::repository::{
        MockOnCallOverrideRepository, MockOnCallScheduleRepository, MockRotationLayerRepository,
        MockRotationParticipantRepository,
    };
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::escalation::scheduler::MockEscalationScheduler;
    use rim_services::events::Observers;
    use rim_services::incident::command::{IngestAlertOutcome, ListIncidentsQuery};
    use rim_services::incident::dto::IncidentListPageCursor;
    use rim_services::incident::service::IncidentService;
    use rim_services::message::service::MockMessageService;
    use rim_services::{Actor, ServiceError};

    /// Every unit test exercises repo/permission logic, not the escalation timer or mention/
    /// email sending — a fresh `MockMessageService`/`MockEscalationScheduler` with no
    /// expectations set is correct: any unexpected call panics loudly (mockall's default),
    /// which is exactly the guard we want for tests that don't declare/ack/fire an escalation.
    fn no_op_message_service() -> Arc<dyn rim_services::message::service::MessageService> {
        Arc::new(MockMessageService::new())
    }

    fn no_op_escalation_scheduler()
    -> Arc<dyn rim_services::escalation::scheduler::EscalationScheduler> {
        Arc::new(MockEscalationScheduler::new())
    }

    use crate::IncidentRepos;
    use crate::incident::DefaultIncidentService;

    fn make_incident(server_id: ServerId, status: IncidentStatus, n: i64) -> Incident {
        let ts = Utc.timestamp_opt(1_700_000_000 + n, 0).unwrap();
        Incident {
            id: IncidentId::new(),
            server_id,
            conversation_id: ConversationId::new(),
            started_by: UserId::new(),
            status,
            severity: Some(IncidentSeverity::Sev2),
            title: format!("incident #{n}"),
            original_topic: None,
            started_at: ts,
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: ts,
            updated_at: ts,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
            acknowledged_at: None,
            acknowledged_by: None,
        }
    }

    fn build_service(
        incident_repo: MockIncidentRepository,
        server_member_repo: MockServerMemberRepository,
    ) -> DefaultIncidentService {
        DefaultIncidentService::new(
            IncidentRepos {
                incident_repo: Arc::new(incident_repo),
                incident_phase_repo: Arc::new(MockIncidentPhaseRepository::new()),
                incident_message_tag_repo: Arc::new(MockIncidentMessageTagRepository::new()),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
                escalation_run_repo: Arc::new(MockEscalationRunRepository::new()),
                escalation_policy_repo: Arc::new(MockEscalationPolicyRepository::new()),
                escalation_step_repo: Arc::new(MockEscalationStepRepository::new()),
                conversation_escalation_policy_repo: Arc::new(
                    MockConversationEscalationPolicyRepository::new(),
                ),
                on_call_schedule_repo: Arc::new(MockOnCallScheduleRepository::new()),
                rotation_layer_repo: Arc::new(MockRotationLayerRepository::new()),
                rotation_participant_repo: Arc::new(MockRotationParticipantRepository::new()),
                on_call_override_repo: Arc::new(MockOnCallOverrideRepository::new()),
            },
            None,
            Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
            Arc::new(crate::email::LogEmailSender),
            None,
            no_op_message_service(),
            no_op_escalation_scheduler(),
        )
    }

    #[tokio::test]
    async fn list_incidents_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .with(eq(server_id), eq(actor_id))
            .returning(|_, _| Ok(false));

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo.expect_list_incidents().never();

        let svc = build_service(incident_repo, server_member_repo);

        let err = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: None,
                severity_filter: None,
                dir: PageDir::After,
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_incidents_passes_filters_and_maps_page() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let i1 = make_incident(server_id, IncidentStatus::Active, 2);
        let i2 = make_incident(server_id, IncidentStatus::Active, 1);
        let next_cursor = IncidentListCursor {
            created_at: i2.created_at,
            incident_id: i2.id,
        };

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_list_incidents()
            .withf(move |sid, statuses, severities, limit, dir, cursor| {
                *sid == server_id
                    && *statuses == Some(&[IncidentStatus::Active][..])
                    && *severities == Some(&[IncidentSeverity::Sev1, IncidentSeverity::Sev2][..])
                    && *limit == 20
                    && matches!(dir, PageDir::After)
                    && cursor.is_none()
            })
            .returning(move |_, _, _, _, _, _| {
                let i1 = i1.clone();
                let i2 = i2.clone();
                Box::pin(async move { Ok(Page::new(vec![i1, i2], Some(next_cursor), None)) })
            });

        let svc = build_service(incident_repo, server_member_repo);

        let page = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: Some(vec![IncidentStatus::Active]),
                severity_filter: Some(vec![IncidentSeverity::Sev1, IncidentSeverity::Sev2]),
                dir: PageDir::After,
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap();

        assert_eq!(page.items.len(), 2);
        let next: IncidentListPageCursor = page.next.unwrap();
        assert_eq!(next.incident_id, next_cursor.incident_id);
        assert_eq!(next.created_at, next_cursor.created_at);
        assert!(page.prev.is_none());
    }

    #[tokio::test]
    async fn list_incidents_forwards_cursor_and_dir() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let cursor = IncidentListPageCursor {
            created_at: Utc.timestamp_opt(1_700_000_500, 0).unwrap(),
            incident_id: IncidentId::new(),
        };

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut incident_repo = MockIncidentRepository::new();
        let cursor_for_check = cursor;
        incident_repo
            .expect_list_incidents()
            .withf(move |_, statuses, severities, _, dir, c| {
                statuses.is_none()
                    && severities.is_none()
                    && matches!(dir, PageDir::Before)
                    && c.map(|c| {
                        c.incident_id == cursor_for_check.incident_id
                            && c.created_at == cursor_for_check.created_at
                    })
                    .unwrap_or(false)
            })
            .returning(|_, _, _, _, _, _| {
                Box::pin(async move { Ok(Page::new(vec![], None, None)) })
            });

        let svc = build_service(incident_repo, server_member_repo);

        let page = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: None,
                severity_filter: None,
                dir: PageDir::Before,
                cursor: Some(cursor),
                limit: 20,
            })
            .await
            .unwrap();
        assert!(page.items.is_empty());
    }

    fn build_ingest_service(
        incident_repo: MockIncidentRepository,
        incident_phase_repo: MockIncidentPhaseRepository,
        conversation_repo: MockConversationRepository,
        server_member_repo: MockServerMemberRepository,
    ) -> DefaultIncidentService {
        // `ingest_alert` unconditionally resolves the channel's default escalation policy on
        // every new-incident path, and unconditionally looks up (and no-ops on) an active
        // escalation run on every resolved-incident path — neither is under test in the
        // `ingest_alert_*` suite below, so both mocks return "nothing configured" by default.
        let mut conversation_escalation_policy_repo =
            MockConversationEscalationPolicyRepository::new();
        conversation_escalation_policy_repo
            .expect_get()
            .returning(|_| Box::pin(async { Ok(None) }));
        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_active_by_incident()
            .returning(|_| Box::pin(async { Ok(None) }));

        DefaultIncidentService::new(
            IncidentRepos {
                incident_repo: Arc::new(incident_repo),
                incident_phase_repo: Arc::new(incident_phase_repo),
                incident_message_tag_repo: Arc::new(MockIncidentMessageTagRepository::new()),
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
                escalation_run_repo: Arc::new(escalation_run_repo),
                escalation_policy_repo: Arc::new(MockEscalationPolicyRepository::new()),
                escalation_step_repo: Arc::new(MockEscalationStepRepository::new()),
                conversation_escalation_policy_repo: Arc::new(conversation_escalation_policy_repo),
                on_call_schedule_repo: Arc::new(MockOnCallScheduleRepository::new()),
                rotation_layer_repo: Arc::new(MockRotationLayerRepository::new()),
                rotation_participant_repo: Arc::new(MockRotationParticipantRepository::new()),
                on_call_override_repo: Arc::new(MockOnCallOverrideRepository::new()),
            },
            None,
            Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
            Arc::new(crate::email::LogEmailSender),
            None,
            no_op_message_service(),
            no_op_escalation_scheduler(),
        )
    }

    fn make_channel(
        server_id: ServerId,
        id: ConversationId,
    ) -> rim_core::conversation::model::Conversation {
        rim_core::conversation::model::Conversation {
            id,
            conversation_type: rim_core::conversation::model::ConversationType::Channel,
            title: Some("alerts".to_string()),
            topic: None,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            server_id,
        }
    }

    fn make_phase(incident_id: IncidentId) -> rim_core::incident::model::IncidentPhaseRecord {
        rim_core::incident::model::IncidentPhaseRecord {
            id: rim_core::incident::model::IncidentPhaseId::new(),
            incident_id,
            phase: rim_core::incident::model::IncidentPhaseName::Detection,
            started_at: Utc::now(),
            ended_at: None,
            note: None,
        }
    }

    fn make_alert_event(
        fingerprint: &str,
        status: rim_services::alert::AlertStatus,
    ) -> rim_services::alert::AlertEvent {
        rim_services::alert::AlertEvent {
            fingerprint: fingerprint.to_string(),
            status,
            title: "high CPU".to_string(),
            severity: Some(IncidentSeverity::Sev2),
            source: rim_services::alert::AlertSource::Generic,
            summary: Some("CPU > 90% for 5m".to_string()),
            started_at: None,
            labels: Default::default(),
        }
    }

    fn ingest_cmd(
        actor_id: UserId,
        server_id: ServerId,
        conversation_id: ConversationId,
        event: rim_services::alert::AlertEvent,
    ) -> rim_services::incident::command::IngestAlertCmd {
        rim_services::incident::command::IngestAlertCmd {
            actor: Actor::new(actor_id),
            server_id,
            conversation_id,
            source: rim_services::alert::AlertSource::Generic,
            event,
        }
    }

    #[tokio::test]
    async fn ingest_alert_requires_server_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        let svc = build_ingest_service(
            MockIncidentRepository::new(),
            MockIncidentPhaseRepository::new(),
            MockConversationRepository::new(),
            server_member_repo,
        );

        let event = make_alert_event("fp1", rim_services::alert::AlertStatus::Firing);
        let err = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn ingest_alert_creates_new_incident_for_new_fingerprint() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });
        conversation_repo
            .expect_update_topic()
            .returning(move |_, _| {
                Box::pin(async move { Ok(make_channel(server_id, conversation_id)) })
            });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        incident_repo
            .expect_find_active_by_conversation()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(Incident {
                    id: IncidentId::new(),
                    server_id: new.server_id,
                    conversation_id: new.conversation_id,
                    started_by: new.started_by,
                    status: IncidentStatus::Active,
                    severity: new.severity,
                    title: new.title,
                    original_topic: new.original_topic,
                    started_at: Utc::now(),
                    mitigated_at: None,
                    resolved_at: None,
                    postmortem_draft: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    alert_fingerprint: new.alert_fingerprint,
                    source: new.source,
                    escalation_policy_id: new.escalation_policy_id,
                    acknowledged_at: None,
                    acknowledged_by: None,
                })
            })
        });

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_create()
            .returning(|incident_id, _, _| Box::pin(async move { Ok(make_phase(incident_id)) }));

        let svc = build_ingest_service(
            incident_repo,
            incident_phase_repo,
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-new", rim_services::alert::AlertStatus::Firing);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        match outcome {
            IngestAlertOutcome::Created(dto) => {
                assert_eq!(dto.alert_fingerprint.as_deref(), Some("fp-new"));
                assert_eq!(dto.source, "generic");
            }
            other => panic!("expected Created, got {other:?}"),
        }
    }

    /// `ingest_alert`'s `(Firing, None)` branch, "different active incident already on this
    /// conversation" sub-case (`incident.rs` ~793-810): a brand-new fingerprint must NOT create a
    /// second incident on top of one that's already active on the same conversation — it should
    /// attach a timeline note to the existing incident instead and report `Updated`.
    #[tokio::test]
    async fn ingest_alert_attaches_to_different_active_incident_on_same_conversation() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let active = make_incident(server_id, IncidentStatus::Active, 1);
        let active_id = active.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        incident_repo
            .expect_find_active_by_conversation()
            .returning(move |_| {
                let active = active.clone();
                Box::pin(async move { Ok(Some(active)) })
            });
        // The "attach a note to the existing incident" path must never try to create a second
        // incident for this conversation.
        incident_repo.expect_create().never();

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_create()
            .withf(move |incident_id, _, _| *incident_id == active_id)
            .returning(|incident_id, _, _| Box::pin(async move { Ok(make_phase(incident_id)) }));

        let svc = build_ingest_service(
            incident_repo,
            incident_phase_repo,
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-brand-new", rim_services::alert::AlertStatus::Firing);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        match outcome {
            IngestAlertOutcome::Updated(dto) => assert_eq!(dto.id, active_id),
            other => {
                panic!("expected Updated (attached to the existing active incident), got {other:?}")
            }
        }
    }

    /// Track B3 §5 — declaring an incident emails every workspace owner, once
    /// `RIM_PUBLIC_BASE_URL` is configured. All other incident tests in this file pass
    /// `public_base_url: None` to the constructor (via `build_service`/`build_ingest_service`),
    /// which short-circuits `notify_owners_of_declared_incident` before it ever touches
    /// `server_member_repo`/`user_repo`/`email_sender` — so this is the only test that exercises
    /// the actual "send" path.
    #[tokio::test]
    async fn ingest_alert_created_notifies_owners_when_base_url_configured() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let owner_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        server_member_repo
            .expect_get_members_for_server()
            .returning(move |_| {
                Ok(vec![rim_core::server_member::model::ServerMember {
                    id: rim_core::server_member::model::ServerMemberId::new(),
                    server_id,
                    user_id: owner_id,
                    member_role: rim_core::server_member::model::ServerMemberRole::Owner,
                    joined_at: Utc::now(),
                }])
            });

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });
        conversation_repo
            .expect_update_topic()
            .returning(move |_, _| {
                Box::pin(async move { Ok(make_channel(server_id, conversation_id)) })
            });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        incident_repo
            .expect_find_active_by_conversation()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(Incident {
                    id: IncidentId::new(),
                    server_id: new.server_id,
                    conversation_id: new.conversation_id,
                    started_by: new.started_by,
                    status: IncidentStatus::Active,
                    severity: new.severity,
                    title: new.title,
                    original_topic: new.original_topic,
                    started_at: Utc::now(),
                    mitigated_at: None,
                    resolved_at: None,
                    postmortem_draft: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    alert_fingerprint: new.alert_fingerprint,
                    source: new.source,
                    escalation_policy_id: new.escalation_policy_id,
                    acknowledged_at: None,
                    acknowledged_by: None,
                })
            })
        });

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_create()
            .returning(|incident_id, _, _| Box::pin(async move { Ok(make_phase(incident_id)) }));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_ids().returning(move |_| {
            let now = Utc::now();
            Ok(vec![rim_core::user::model::User {
                id: owner_id,
                username: "owner".to_string(),
                password_hash: "hash".to_string(),
                created_at: now,
                updated_at: now,
                last_login_at: None,
                display_name: None,
                avatar_url: None,
                status_text: None,
                bio: None,
                email: "owner@example.com".to_string(),
                timezone: None,
            }])
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .withf(|msg| msg.to == "owner@example.com" && msg.subject.contains("SEV"))
            .times(1)
            .returning(|_| Ok(()));

        let mut conversation_escalation_policy_repo =
            MockConversationEscalationPolicyRepository::new();
        conversation_escalation_policy_repo
            .expect_get()
            .returning(|_| Box::pin(async { Ok(None) }));

        let svc = DefaultIncidentService::new(
            IncidentRepos {
                incident_repo: Arc::new(incident_repo),
                incident_phase_repo: Arc::new(incident_phase_repo),
                incident_message_tag_repo: Arc::new(MockIncidentMessageTagRepository::new()),
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(user_repo),
                escalation_run_repo: Arc::new(MockEscalationRunRepository::new()),
                escalation_policy_repo: Arc::new(MockEscalationPolicyRepository::new()),
                escalation_step_repo: Arc::new(MockEscalationStepRepository::new()),
                conversation_escalation_policy_repo: Arc::new(conversation_escalation_policy_repo),
                on_call_schedule_repo: Arc::new(MockOnCallScheduleRepository::new()),
                rotation_layer_repo: Arc::new(MockRotationLayerRepository::new()),
                rotation_participant_repo: Arc::new(MockRotationParticipantRepository::new()),
                on_call_override_repo: Arc::new(MockOnCallOverrideRepository::new()),
            },
            None,
            Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
            no_op_message_service(),
            no_op_escalation_scheduler(),
        );

        let event = make_alert_event("fp-notify", rim_services::alert::AlertStatus::Firing);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();
        assert!(matches!(outcome, IngestAlertOutcome::Created(_)));
    }

    #[tokio::test]
    async fn ingest_alert_falls_back_to_updated_on_concurrent_fingerprint_conflict() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let winner = make_incident(server_id, IncidentStatus::Active, 1);
        let winner_id = winner.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });

        // First lookup (the TOCTOU pre-check): nobody has it yet. Second lookup (after the
        // conflicting create): the concurrent request's incident is now visible.
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut incident_repo = MockIncidentRepository::new();
        incident_repo.expect_find_by_fingerprint().returning({
            let call_count = std::sync::Arc::clone(&call_count);
            let winner = winner.clone();
            move |_, _| {
                let n = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let winner = winner.clone();
                Box::pin(async move { if n == 0 { Ok(None) } else { Ok(Some(winner)) } })
            }
        });
        incident_repo
            .expect_find_active_by_conversation()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_repo.expect_create().returning(|_| {
            Box::pin(async {
                Err(rim_core::repository::error::RepoError::new(
                    rim_core::repository::error::RepoErrorKind::Conflict,
                ))
            })
        });

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_create()
            .withf(move |incident_id, _, _| *incident_id == winner_id)
            .returning(|incident_id, _, _| Box::pin(async move { Ok(make_phase(incident_id)) }));

        let svc = build_ingest_service(
            incident_repo,
            incident_phase_repo,
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-race", rim_services::alert::AlertStatus::Firing);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        match outcome {
            IngestAlertOutcome::Updated(dto) => assert_eq!(dto.id, winner_id),
            other => panic!("expected Updated (conflict fallback), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_alert_updates_existing_incident_for_repeat_fingerprint() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let existing = make_incident(server_id, IncidentStatus::Active, 1);
        let existing_id = existing.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(move |_, _| {
                let existing = existing.clone();
                Box::pin(async move { Ok(Some(existing)) })
            });
        incident_repo.expect_create().never();

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_find_current_phase()
            .returning(|incident_id| Box::pin(async move { Ok(Some(make_phase(incident_id))) }));
        incident_phase_repo
            .expect_create()
            .withf(move |incident_id, _, _| *incident_id == existing_id)
            .returning(|incident_id, _, _| Box::pin(async move { Ok(make_phase(incident_id)) }));

        let svc = build_ingest_service(
            incident_repo,
            incident_phase_repo,
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-repeat", rim_services::alert::AlertStatus::Firing);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        match outcome {
            IngestAlertOutcome::Updated(dto) => assert_eq!(dto.id, existing_id),
            other => panic!("expected Updated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_alert_resolves_existing_incident() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let existing = make_incident(server_id, IncidentStatus::Active, 1);
        let existing_id = existing.id;

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });
        conversation_repo
            .expect_update_topic()
            .returning(move |_, _| {
                Box::pin(async move { Ok(make_channel(server_id, conversation_id)) })
            });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(move |_, _| {
                let existing = existing.clone();
                Box::pin(async move { Ok(Some(existing)) })
            });
        incident_repo
            .expect_update_status()
            .withf(move |id, status, _, _| {
                *id == existing_id && *status == IncidentStatus::Resolved
            })
            .returning(move |id, status, mitigated_at, resolved_at| {
                Box::pin(async move {
                    Ok(Incident {
                        id,
                        server_id,
                        conversation_id,
                        started_by: UserId::new(),
                        status,
                        severity: Some(IncidentSeverity::Sev2),
                        title: "high CPU".to_string(),
                        original_topic: None,
                        started_at: Utc::now(),
                        mitigated_at,
                        resolved_at,
                        postmortem_draft: None,
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        alert_fingerprint: Some("fp-resolve".to_string()),
                        source: "generic".to_string(),
                        escalation_policy_id: None,
                        acknowledged_at: None,
                        acknowledged_by: None,
                    })
                })
            });

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_find_current_phase()
            .returning(|incident_id| Box::pin(async move { Ok(Some(make_phase(incident_id))) }));
        incident_phase_repo
            .expect_end_phase()
            .returning(|phase_id| {
                Box::pin(async move {
                    let mut p = make_phase(IncidentId::new());
                    p.id = phase_id;
                    p.ended_at = Some(Utc::now());
                    Ok(p)
                })
            });

        let svc = build_ingest_service(
            incident_repo,
            incident_phase_repo,
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-resolve", rim_services::alert::AlertStatus::Resolved);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        match outcome {
            IngestAlertOutcome::Resolved(dto) => {
                assert_eq!(dto.id, existing_id);
                assert_eq!(dto.status, IncidentStatus::Resolved);
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_alert_ignores_resolved_alert_with_no_matching_incident() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move { Ok(Some(make_channel(server_id, conversation_id))) })
        });

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_find_by_fingerprint()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        incident_repo.expect_update_status().never();
        incident_repo.expect_create().never();

        let svc = build_ingest_service(
            incident_repo,
            MockIncidentPhaseRepository::new(),
            conversation_repo,
            server_member_repo,
        );

        let event = make_alert_event("fp-unknown", rim_services::alert::AlertStatus::Resolved);
        let outcome = svc
            .ingest_alert(ingest_cmd(actor_id, server_id, conversation_id, event))
            .await
            .unwrap();

        assert!(matches!(outcome, IngestAlertOutcome::Ignored));
    }

    // ── Escalation chain tests (C1b §7, mocked scheduler + repos) ───────────

    use rim_core::escalation::model::{
        EscalationAction, EscalationPolicyId, EscalationRun, EscalationRunId, EscalationRunStatus,
        EscalationStep, EscalationStepId, EscalationTarget,
    };
    use rim_core::oncall::model::OnCallScheduleId;
    use rim_services::incident::command::{AcknowledgeIncidentCmd, UpdateIncidentStatusCmd};

    #[allow(clippy::too_many_arguments)]
    fn build_escalation_service(
        incident_repo: MockIncidentRepository,
        escalation_run_repo: MockEscalationRunRepository,
        escalation_step_repo: MockEscalationStepRepository,
        on_call_schedule_repo: MockOnCallScheduleRepository,
        message_service: Arc<dyn rim_services::message::service::MessageService>,
        escalation_scheduler: Arc<dyn rim_services::escalation::scheduler::EscalationScheduler>,
        incident_phase_repo: MockIncidentPhaseRepository,
        server_member_repo: MockServerMemberRepository,
        conversation_repo: MockConversationRepository,
    ) -> DefaultIncidentService {
        DefaultIncidentService::new(
            IncidentRepos {
                incident_repo: Arc::new(incident_repo),
                incident_phase_repo: Arc::new(incident_phase_repo),
                incident_message_tag_repo: Arc::new(MockIncidentMessageTagRepository::new()),
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
                user_repo: Arc::new(MockUserRepository::new()),
                escalation_run_repo: Arc::new(escalation_run_repo),
                escalation_policy_repo: Arc::new(MockEscalationPolicyRepository::new()),
                escalation_step_repo: Arc::new(escalation_step_repo),
                conversation_escalation_policy_repo: Arc::new(
                    MockConversationEscalationPolicyRepository::new(),
                ),
                on_call_schedule_repo: Arc::new(on_call_schedule_repo),
                rotation_layer_repo: Arc::new(MockRotationLayerRepository::new()),
                rotation_participant_repo: Arc::new(MockRotationParticipantRepository::new()),
                on_call_override_repo: Arc::new(MockOnCallOverrideRepository::new()),
            },
            None,
            Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
            Arc::new(crate::email::LogEmailSender),
            None,
            message_service,
            escalation_scheduler,
        )
    }

    fn make_run(
        id: EscalationRunId,
        incident_id: IncidentId,
        policy_id: EscalationPolicyId,
        current_step_index: i32,
        status: EscalationRunStatus,
    ) -> EscalationRun {
        let now = Utc::now();
        EscalationRun {
            id,
            incident_id,
            policy_id,
            current_step_index,
            next_fire_at: Some(now),
            status,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_step(
        policy_id: EscalationPolicyId,
        position: i32,
        delay_seconds: i64,
        target: EscalationTarget,
        actions: Vec<EscalationAction>,
    ) -> EscalationStep {
        EscalationStep {
            id: EscalationStepId::new(),
            policy_id,
            position,
            delay_seconds,
            target,
            actions,
        }
    }

    /// declare → step0 fires → step1 armed at correct `next_fire_at` (§7 bullet 1).
    #[tokio::test]
    async fn fire_escalation_step_advances_to_next_step_with_correct_fire_at() {
        let run_id = EscalationRunId::new();
        let incident_id = IncidentId::new();
        let policy_id = EscalationPolicyId::new();
        let target_user = UserId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident = make_incident(ServerId::new(), IncidentStatus::Active, 1);
        let incident_for_lookup = Incident {
            id: incident_id,
            ..incident
        };
        incident_repo
            .expect_find_by_id()
            .withf(move |id| *id == incident_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(incident_for_lookup)) }));

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_by_id()
            .withf(move |id| *id == run_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Active,
                    )))
                })
            });
        escalation_run_repo
            .expect_advance()
            .withf(move |id, next_index, fire_at| {
                *id == run_id && *next_index == 1 && fire_at.is_some()
            })
            .times(1)
            .return_once(move |id, next_index, fire_at| {
                Box::pin(async move {
                    let mut r = make_run(
                        id,
                        incident_id,
                        policy_id,
                        next_index,
                        EscalationRunStatus::Active,
                    );
                    r.next_fire_at = fire_at;
                    Ok(r)
                })
            });

        let mut escalation_step_repo = MockEscalationStepRepository::new();
        escalation_step_repo
            .expect_list_by_policy()
            .withf(move |id| *id == policy_id)
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![
                        make_step(
                            policy_id,
                            0,
                            0,
                            EscalationTarget::User(target_user),
                            vec![EscalationAction::MentionInChannel],
                        ),
                        make_step(
                            policy_id,
                            1,
                            300,
                            EscalationTarget::User(target_user),
                            vec![EscalationAction::Email],
                        ),
                    ])
                })
            });

        let mut message_service = MockMessageService::new();
        // Best-effort action: a send failure (e.g. started_by left the server) must not abort
        // the chain (§5) — the test asserts `advance` still fires below.
        message_service
            .expect_send_message()
            .return_once(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_find_current_phase()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_phase_repo
            .expect_create()
            .returning(|incident_id, phase, note| {
                Box::pin(async move {
                    Ok(rim_core::incident::model::IncidentPhaseRecord {
                        id: rim_core::incident::model::IncidentPhaseId::new(),
                        incident_id,
                        phase,
                        started_at: Utc::now(),
                        ended_at: None,
                        note,
                    })
                })
            });

        let mut escalation_scheduler = MockEscalationScheduler::new();
        escalation_scheduler
            .expect_arm()
            .withf(move |id, _fire_at| *id == run_id)
            .times(1)
            .return_const(());

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        // The page-time membership recheck (C1 security review) — `target_user` is still a
        // current member, so it passes through unfiltered.
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let svc = build_escalation_service(
            incident_repo,
            escalation_run_repo,
            escalation_step_repo,
            MockOnCallScheduleRepository::new(),
            Arc::new(message_service),
            Arc::new(escalation_scheduler),
            incident_phase_repo,
            server_member_repo,
            conversation_repo,
        );

        svc.fire_escalation_step(run_id)
            .await
            .expect("fire_escalation_step must succeed");
    }

    /// Page-time membership recheck (C1 security review, MEDIUM defense-in-depth): a target
    /// user who has since left the server must not be mentioned or emailed, even though they
    /// resolved from the step target. `message_service` is left with zero expectations — any
    /// call to it (mention or, indirectly via the email branch, `user_repo`) panics, which is
    /// exactly the assertion that both actions were skipped. The chain must still advance
    /// (`escalation_run_repo.advance` is asserted `times(1)`) — a departed target degrades
    /// exactly like an empty/deleted on-call schedule (§5), it doesn't abort the run.
    #[tokio::test]
    async fn fire_escalation_step_skips_actions_for_target_no_longer_a_member() {
        let run_id = EscalationRunId::new();
        let incident_id = IncidentId::new();
        let policy_id = EscalationPolicyId::new();
        let server_id = ServerId::new();
        let departed_user = UserId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident_for_lookup = Incident {
            id: incident_id,
            ..make_incident(server_id, IncidentStatus::Active, 6)
        };
        incident_repo
            .expect_find_by_id()
            .withf(move |id| *id == incident_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(incident_for_lookup)) }));

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_by_id()
            .withf(move |id| *id == run_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Active,
                    )))
                })
            });
        escalation_run_repo
            .expect_advance()
            .withf(move |id, next_index, fire_at| {
                *id == run_id && *next_index == 1 && fire_at.is_some()
            })
            .times(1)
            .return_once(move |id, next_index, fire_at| {
                Box::pin(async move {
                    let mut r = make_run(
                        id,
                        incident_id,
                        policy_id,
                        next_index,
                        EscalationRunStatus::Active,
                    );
                    r.next_fire_at = fire_at;
                    Ok(r)
                })
            });

        let mut escalation_step_repo = MockEscalationStepRepository::new();
        escalation_step_repo
            .expect_list_by_policy()
            .withf(move |id| *id == policy_id)
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![
                        make_step(
                            policy_id,
                            0,
                            0,
                            EscalationTarget::User(departed_user),
                            vec![EscalationAction::MentionInChannel, EscalationAction::Email],
                        ),
                        make_step(
                            policy_id,
                            1,
                            300,
                            EscalationTarget::User(departed_user),
                            vec![EscalationAction::Email],
                        ),
                    ])
                })
            });

        // No expectations on message_service — a mention/email call panics.
        let message_service = MockMessageService::new();

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_find_current_phase()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_phase_repo
            .expect_create()
            .returning(|incident_id, phase, note| {
                Box::pin(async move {
                    Ok(rim_core::incident::model::IncidentPhaseRecord {
                        id: rim_core::incident::model::IncidentPhaseId::new(),
                        incident_id,
                        phase,
                        started_at: Utc::now(),
                        ended_at: None,
                        note,
                    })
                })
            });

        let mut escalation_scheduler = MockEscalationScheduler::new();
        escalation_scheduler
            .expect_arm()
            .withf(move |id, _fire_at| *id == run_id)
            .times(1)
            .return_const(());

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        // `departed_user` has left `server_id` since the step was authored/last resolved.
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .withf(move |sid, uid| *sid == server_id && *uid == departed_user)
            .returning(|_, _| Ok(false));

        let svc = build_escalation_service(
            incident_repo,
            escalation_run_repo,
            escalation_step_repo,
            MockOnCallScheduleRepository::new(),
            Arc::new(message_service),
            Arc::new(escalation_scheduler),
            incident_phase_repo,
            server_member_repo,
            conversation_repo,
        );

        svc.fire_escalation_step(run_id)
            .await
            .expect("fire_escalation_step must succeed even when the target is filtered out");
    }

    /// A fire after cancel is a no-op (status-reload guard, §7 bullet 4). Leaving
    /// `incident_repo`/`escalation_step_repo` with zero expectations means any further call
    /// panics loudly — the strongest possible assertion that the guard short-circuits.
    #[tokio::test]
    async fn fire_escalation_step_on_cancelled_run_is_a_no_op() {
        let run_id = EscalationRunId::new();
        let incident_id = IncidentId::new();
        let policy_id = EscalationPolicyId::new();

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_by_id()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Cancelled,
                    )))
                })
            });

        let svc = build_escalation_service(
            MockIncidentRepository::new(),
            escalation_run_repo,
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            no_op_escalation_scheduler(),
            MockIncidentPhaseRepository::new(),
            MockServerMemberRepository::new(),
            MockConversationRepository::new(),
        );

        svc.fire_escalation_step(run_id)
            .await
            .expect("firing a cancelled run must be a no-op success, not an error");
    }

    /// An `EscalationTarget::OnCall` resolving to an empty/deleted schedule skips actions but
    /// still advances the chain (§7 bullet 5). `message_service`/`user_repo` are left with zero
    /// expectations — if the (empty) target were mistakenly treated as non-empty, the action
    /// loop would call one of them and panic.
    #[tokio::test]
    async fn fire_escalation_step_on_call_target_empty_schedule_skips_actions_but_advances() {
        let run_id = EscalationRunId::new();
        let incident_id = IncidentId::new();
        let policy_id = EscalationPolicyId::new();
        let schedule_id = OnCallScheduleId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident = Incident {
            id: incident_id,
            ..make_incident(ServerId::new(), IncidentStatus::Active, 2)
        };
        incident_repo
            .expect_find_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(incident)) }));

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_by_id()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Active,
                    )))
                })
            });
        escalation_run_repo
            .expect_advance()
            .withf(move |id, next_index, _| *id == run_id && *next_index == 1)
            .times(1)
            .return_once(move |id, next_index, fire_at| {
                Box::pin(async move {
                    let mut r = make_run(
                        id,
                        incident_id,
                        policy_id,
                        next_index,
                        EscalationRunStatus::Active,
                    );
                    r.next_fire_at = fire_at;
                    Ok(r)
                })
            });

        let mut escalation_step_repo = MockEscalationStepRepository::new();
        escalation_step_repo
            .expect_list_by_policy()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![
                        make_step(
                            policy_id,
                            0,
                            0,
                            EscalationTarget::OnCall(schedule_id),
                            vec![EscalationAction::MentionInChannel, EscalationAction::Email],
                        ),
                        make_step(
                            policy_id,
                            1,
                            60,
                            EscalationTarget::OnCall(schedule_id),
                            vec![EscalationAction::Email],
                        ),
                    ])
                })
            });

        let mut on_call_schedule_repo = MockOnCallScheduleRepository::new();
        // The schedule referenced by the step no longer exists (deleted after authoring).
        on_call_schedule_repo
            .expect_find_by_id()
            .withf(move |id| *id == schedule_id)
            .returning(|_| Box::pin(async { Ok(None) }));

        let mut incident_phase_repo = MockIncidentPhaseRepository::new();
        incident_phase_repo
            .expect_find_current_phase()
            .returning(|_| Box::pin(async { Ok(None) }));
        incident_phase_repo
            .expect_create()
            .returning(|incident_id, phase, note| {
                Box::pin(async move {
                    Ok(rim_core::incident::model::IncidentPhaseRecord {
                        id: rim_core::incident::model::IncidentPhaseId::new(),
                        incident_id,
                        phase,
                        started_at: Utc::now(),
                        ended_at: None,
                        note,
                    })
                })
            });

        let mut escalation_scheduler = MockEscalationScheduler::new();
        escalation_scheduler.expect_arm().times(1).return_const(());

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        // No expectations on message_service/user_repo — a call to either panics.
        let svc = build_escalation_service(
            incident_repo,
            escalation_run_repo,
            escalation_step_repo,
            on_call_schedule_repo,
            no_op_message_service(),
            Arc::new(escalation_scheduler),
            incident_phase_repo,
            MockServerMemberRepository::new(),
            conversation_repo,
        );

        svc.fire_escalation_step(run_id)
            .await
            .expect("empty on-call target must still advance the chain");
    }

    /// Ack cancels the active escalation run (§4/§7 bullet 2).
    #[tokio::test]
    async fn acknowledge_incident_cancels_active_escalation_run() {
        let incident_id = IncidentId::new();
        let run_id = EscalationRunId::new();
        let policy_id = EscalationPolicyId::new();
        let actor_id = UserId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident = Incident {
            id: incident_id,
            acknowledged_at: None,
            ..make_incident(ServerId::new(), IncidentStatus::Active, 3)
        };
        incident_repo
            .expect_find_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(incident)) }));
        incident_repo
            .expect_acknowledge()
            .return_once(move |id, at, by| {
                Box::pin(async move {
                    Ok(Incident {
                        acknowledged_at: Some(at),
                        acknowledged_by: Some(by),
                        ..Incident {
                            id,
                            ..make_incident(ServerId::new(), IncidentStatus::Active, 3)
                        }
                    })
                })
            });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_active_by_incident()
            .withf(move |id| *id == incident_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Active,
                    )))
                })
            });
        escalation_run_repo
            .expect_set_status()
            .withf(move |id, status| *id == run_id && *status == EscalationRunStatus::Cancelled)
            .times(1)
            .return_once(move |id, status| {
                Box::pin(async move { Ok(make_run(id, incident_id, policy_id, 0, status)) })
            });

        let mut escalation_scheduler = MockEscalationScheduler::new();
        escalation_scheduler
            .expect_cancel()
            .withf(move |id| *id == run_id)
            .times(1)
            .return_const(());

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(|_| Box::pin(async { Ok(None) }));

        let svc = build_escalation_service(
            incident_repo,
            escalation_run_repo,
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            Arc::new(escalation_scheduler),
            MockIncidentPhaseRepository::new(),
            server_member_repo,
            conversation_repo,
        );

        svc.acknowledge_incident(AcknowledgeIncidentCmd {
            actor: Actor::new(actor_id),
            incident_id,
        })
        .await
        .expect("acknowledge_incident must succeed");
    }

    /// Idempotent: acking an already-acked incident is a no-op success (§4) — the escalation run
    /// lookup/cancel must NOT be attempted a second time.
    #[tokio::test]
    async fn acknowledge_incident_is_idempotent_when_already_acked() {
        let incident_id = IncidentId::new();
        let actor_id = UserId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident = Incident {
            id: incident_id,
            acknowledged_at: Some(Utc::now()),
            acknowledged_by: Some(UserId::new()),
            ..make_incident(ServerId::new(), IncidentStatus::Active, 4)
        };
        incident_repo
            .expect_find_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(incident)) }));
        // No `expect_acknowledge()` — a second ack must not write again.

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        // No expectations on escalation_run_repo — cancellation must not be attempted again.
        let svc = build_escalation_service(
            incident_repo,
            MockEscalationRunRepository::new(),
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            no_op_escalation_scheduler(),
            MockIncidentPhaseRepository::new(),
            server_member_repo,
            MockConversationRepository::new(),
        );

        let dto = svc
            .acknowledge_incident(AcknowledgeIncidentCmd {
                actor: Actor::new(actor_id),
                incident_id,
            })
            .await
            .expect("idempotent ack must succeed");
        assert!(dto.acknowledged_at.is_some());
    }

    /// Mitigating an incident cancels its active escalation run (§4/§6/§7 bullet 3).
    #[tokio::test]
    async fn update_incident_status_to_mitigated_cancels_active_escalation_run() {
        let incident_id = IncidentId::new();
        let run_id = EscalationRunId::new();
        let policy_id = EscalationPolicyId::new();
        let actor_id = UserId::new();

        let mut incident_repo = MockIncidentRepository::new();
        let incident = Incident {
            id: incident_id,
            ..make_incident(ServerId::new(), IncidentStatus::Active, 5)
        };
        incident_repo
            .expect_find_by_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(incident)) }));
        incident_repo.expect_update_status().returning(
            move |id, status, mitigated_at, resolved_at| {
                Box::pin(async move {
                    Ok(Incident {
                        mitigated_at,
                        resolved_at,
                        status,
                        ..Incident {
                            id,
                            ..make_incident(ServerId::new(), IncidentStatus::Active, 5)
                        }
                    })
                })
            },
        );

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_active_by_incident()
            .withf(move |id| *id == incident_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        0,
                        EscalationRunStatus::Active,
                    )))
                })
            });
        escalation_run_repo
            .expect_set_status()
            .withf(move |id, status| *id == run_id && *status == EscalationRunStatus::Cancelled)
            .times(1)
            .return_once(move |id, status| {
                Box::pin(async move { Ok(make_run(id, incident_id, policy_id, 0, status)) })
            });

        let mut escalation_scheduler = MockEscalationScheduler::new();
        escalation_scheduler
            .expect_cancel()
            .withf(move |id| *id == run_id)
            .times(1)
            .return_const(());

        let svc = build_escalation_service(
            incident_repo,
            escalation_run_repo,
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            Arc::new(escalation_scheduler),
            MockIncidentPhaseRepository::new(),
            server_member_repo,
            MockConversationRepository::new(),
        );

        svc.update_incident_status(UpdateIncidentStatusCmd {
            actor: Actor::new(actor_id),
            incident_id,
            status: IncidentStatus::Mitigated,
        })
        .await
        .expect("update_incident_status to Mitigated must succeed");
    }

    // ── get_active_escalation_run (C1c, backs Incident.escalationRun) ───────

    #[tokio::test]
    async fn get_active_escalation_run_returns_the_active_run() {
        let incident_id = IncidentId::new();
        let run_id = EscalationRunId::new();
        let policy_id = EscalationPolicyId::new();

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_active_by_incident()
            .withf(move |id| *id == incident_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(Some(make_run(
                        run_id,
                        incident_id,
                        policy_id,
                        2,
                        EscalationRunStatus::Active,
                    )))
                })
            });

        let svc = build_escalation_service(
            MockIncidentRepository::new(),
            escalation_run_repo,
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            no_op_escalation_scheduler(),
            MockIncidentPhaseRepository::new(),
            MockServerMemberRepository::new(),
            MockConversationRepository::new(),
        );

        let dto = svc
            .get_active_escalation_run(incident_id)
            .await
            .expect("must succeed")
            .expect("an active run exists and must be returned");
        assert_eq!(dto.id, run_id);
        assert_eq!(dto.incident_id, incident_id);
        assert_eq!(dto.policy_id, policy_id);
        assert_eq!(dto.current_step_index, 2);
        assert_eq!(dto.status, EscalationRunStatus::Active);
    }

    /// No active run (never armed, or already completed/cancelled) is `None`, not an error — the
    /// `Incident.escalationRun` field is nullable specifically for this case (§10 FLAG C).
    #[tokio::test]
    async fn get_active_escalation_run_returns_none_when_no_active_run() {
        let incident_id = IncidentId::new();

        let mut escalation_run_repo = MockEscalationRunRepository::new();
        escalation_run_repo
            .expect_find_active_by_incident()
            .withf(move |id| *id == incident_id)
            .return_once(|_| Box::pin(async { Ok(None) }));

        let svc = build_escalation_service(
            MockIncidentRepository::new(),
            escalation_run_repo,
            MockEscalationStepRepository::new(),
            MockOnCallScheduleRepository::new(),
            no_op_message_service(),
            no_op_escalation_scheduler(),
            MockIncidentPhaseRepository::new(),
            MockServerMemberRepository::new(),
            MockConversationRepository::new(),
        );

        let result = svc
            .get_active_escalation_run(incident_id)
            .await
            .expect("must succeed");
        assert!(result.is_none());
    }
}
