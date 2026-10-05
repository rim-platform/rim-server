//! Track C2 (`docs/specs/track-c2-agent-operator.md`) integration tests — the
//! four C1 lifecycle commands flipped `ai_callable: true` under the existing
//! agentic confirmation gate.
//!
//! Mirrors the `chat_list_channels_agent_test.rs` end-to-end pattern: drives
//! `DefaultCommandService::execute_command` against a real registry lookup
//! (the live builtins, not synthetic test defs) with mocked domain services.
//!
//! Covers §6.2 (AI surface — no capability grant needed, pins the §2 empty-caps
//! ruling) and §6.3 (agent confirmation gate: Safe is one-shot for every
//! initiator; the three Mutating commands require a confirmation proof for an
//! agentic initiator and re-execute exactly once on a valid proof; human
//! initiators stay one-shot, unchanged from C1).

use std::sync::Arc;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::incident::model::{Incident, IncidentId, IncidentStatus};
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::oncall::model::{OnCallOverrideId, OnCallScheduleId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::confirmation::ConfirmationProof;
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto, CommandSurfaceDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::incident::dto::IncidentDto;
use rim_services::incident::service::MockIncidentService;
use rim_services::message::service::MockMessageService;
use rim_services::message_reaction::service::MockMessageReactionService;
use rim_services::oncall::dto::{OnCallNowDto, OnCallOverrideDto};
use rim_services::oncall::service::MockOnCallService;
use rim_services::pin::service::MockPinService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::server_member::service::MockServerMemberService;
use rim_services::user::service::MockUserService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

/// A fixed workspace context. The four C2 commands belong to the `incidents` workspace feature
/// (Track 0.4), so they only list/execute with a workspace context; the mocked feature service
/// reports every feature enabled. Fixed (not random) so a confirmation proof issued for one call
/// verifies against the identical context on the retry.
fn workspace_ctx() -> Option<CommandContextDto> {
    Some(CommandContextDto {
        workspace_id: Some(ServerId::from(uuid::Uuid::from_u128(1))),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    })
}

fn actor() -> Actor {
    Actor::new(UserId::new())
}

fn build_service(
    capability_service: MockCapabilityService,
    incident_service: MockIncidentService,
    oncall_service: MockOnCallService,
) -> DefaultCommandService {
    let mut user_repo = MockUserCommandRepository::new();
    user_repo
        .expect_list_for_user()
        .returning(|_| Box::pin(async { Ok(vec![]) }));

    DefaultCommandService::new(
        CommandRepos {
            user_command_repo: Arc::new(user_repo),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(capability_service),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(MockMessageReactionService::new()),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: Arc::new(incident_service),
            oncall_service: Arc::new(oncall_service),
            workspace_feature_service: std::sync::Arc::new(
                MockWorkspaceFeatureService::all_enabled(),
            ),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
    )
}

fn make_incident_dto(
    id: IncidentId,
    acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
) -> IncidentDto {
    let now = chrono::Utc::now();
    IncidentDto::from(Incident {
        id,
        server_id: ServerId::new(),
        conversation_id: rim_core::conversation::model::ConversationId::new(),
        started_by: UserId::new(),
        status: IncidentStatus::Active,
        severity: None,
        title: "t".to_string(),
        original_topic: None,
        started_at: now,
        mitigated_at: None,
        resolved_at: None,
        postmortem_draft: None,
        created_at: now,
        updated_at: now,
        alert_fingerprint: None,
        source: "manual".to_string(),
        escalation_policy_id: None,
        acknowledged_at,
        acknowledged_by: acknowledged_at.map(|_| UserId::new()),
    })
}

// ── §6.2: AI surface — the four commands need no capability grant ──────────

/// The four C2 commands all have `required_capabilities: []` (§2 ruling), so
/// they must appear in the `Surface::Ai` listing for any actor even with a
/// completely empty capability-grant environment — no `check`/`check_many`
/// call is expected on the mock at all (empty caps ⇒ nothing to check).
#[tokio::test]
async fn ai_surface_lists_all_four_c2_commands_with_no_capability_grants() {
    // Other ai_callable builtins in the registry (e.g. `chat.post_message`) DO
    // require caps, so `list_commands(Surface::Ai)` still calls `check_many`
    // for the union of every ai_callable command's `required_capabilities` —
    // it is just never called for these four (their `required_capabilities`
    // is `[]`, §2). Denying every requested cap here proves the four C2
    // commands survive on an empty grant environment regardless.
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many().returning(|_, _, caps| {
        let mut out = std::collections::HashMap::with_capacity(caps.len());
        for c in caps {
            out.insert(c.clone(), false);
        }
        Ok(out)
    });

    let svc = build_service(cap, MockIncidentService::new(), MockOnCallService::new());

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: workspace_ctx(),
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("Surface::Ai listing must succeed with no capability grants");

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    for expected in [
        "incident.ack",
        "incident.escalate",
        "oncall.who",
        "oncall.take_shift",
    ] {
        assert!(
            ids.contains(&expected),
            "{expected} must appear in the AI surface with no capability grants (§2 ruling); got: {ids:?}"
        );
    }
}

// ── §6.3: agent confirmation gate ───────────────────────────────────────────

/// `oncall.who` is Safe — one-shot for every initiator, including Agent. No
/// confirmation challenge is ever issued for it.
#[tokio::test]
async fn oncall_who_agent_initiator_is_one_shot_no_challenge() {
    let schedule_id = OnCallScheduleId::new();
    let mut oncall = MockOnCallService::new();
    oncall.expect_who_is_on_call().times(1).returning(move |_| {
        Box::pin(async move {
            Ok(OnCallNowDto {
                schedule_id,
                current_user_id: None,
                shift_ends_at: None,
                next_user_id: None,
            })
        })
    });

    let svc = build_service(
        MockCapabilityService::new(),
        MockIncidentService::new(),
        oncall,
    );

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "oncall.who".to_string(),
            args: Some(serde_json::json!({ "schedule_id": schedule_id.to_string() })),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("oncall.who must succeed for an Agent initiator without a proof");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    assert!(result.confirmation_required.is_none());
}

/// `incident.ack`, Agent initiator, no proof → `ConfirmationRequired` with a
/// populated challenge; re-submit with the valid `InBandToken` proof →
/// `Success`, and `acknowledge_incident` is called exactly once (not on the
/// first, challenge-only call).
#[tokio::test]
async fn incident_ack_agent_initiator_requires_confirmation_then_succeeds_with_valid_proof() {
    let incident_id = IncidentId::new();
    let now = chrono::Utc::now();

    let mut incident_service = MockIncidentService::new();
    incident_service
        .expect_acknowledge_incident()
        .times(1)
        .withf(move |cmd| {
            cmd.incident_id == incident_id && cmd.initiator == Some(CommandInitiator::Agent)
        })
        .return_once(move |_| {
            Box::pin(async move { Ok(make_incident_dto(incident_id, Some(now))) })
        });

    let svc = build_service(
        MockCapabilityService::new(),
        incident_service,
        MockOnCallService::new(),
    );

    let args = Some(serde_json::json!({ "incident_id": incident_id.to_string() }));
    let a = actor();

    let challenge_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "incident.ack".to_string(),
            args: args.clone(),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("first call (no proof) must return Ok(ConfirmationRequired), not Err");

    assert_eq!(
        challenge_result.status,
        CommandResultStatusDto::ConfirmationRequired
    );
    let challenge = challenge_result
        .confirmation_required
        .expect("ConfirmationRequired must carry a populated challenge");

    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let final_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "incident.ack".to_string(),
            args,
            context: workspace_ctx(),
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("re-submit with valid proof must succeed");

    assert_eq!(final_result.status, CommandResultStatusDto::Success);
}

/// `incident.escalate`, Agent initiator, no proof → `ConfirmationRequired`;
/// re-submit with the valid proof → `Success`, `escalate_incident_now` called
/// exactly once.
#[tokio::test]
async fn incident_escalate_agent_initiator_requires_confirmation_then_succeeds_with_valid_proof() {
    let incident_id = IncidentId::new();

    let mut incident_service = MockIncidentService::new();
    incident_service
        .expect_escalate_incident_now()
        .times(1)
        .withf(move |cmd| {
            cmd.incident_id == incident_id && cmd.initiator == Some(CommandInitiator::Agent)
        })
        .return_once(|_| Box::pin(async { Ok(()) }));

    let svc = build_service(
        MockCapabilityService::new(),
        incident_service,
        MockOnCallService::new(),
    );

    let args = Some(serde_json::json!({ "incident_id": incident_id.to_string() }));
    let a = actor();

    let challenge_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "incident.escalate".to_string(),
            args: args.clone(),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("first call (no proof) must return Ok(ConfirmationRequired), not Err");

    assert_eq!(
        challenge_result.status,
        CommandResultStatusDto::ConfirmationRequired
    );
    let challenge = challenge_result
        .confirmation_required
        .expect("ConfirmationRequired must carry a populated challenge");

    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let final_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "incident.escalate".to_string(),
            args,
            context: workspace_ctx(),
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("re-submit with valid proof must succeed");

    assert_eq!(final_result.status, CommandResultStatusDto::Success);
}

/// `oncall.take_shift`, Agent initiator, no proof → `ConfirmationRequired`;
/// re-submit with the valid proof → `Success`, `create_override` called
/// exactly once.
#[tokio::test]
async fn oncall_take_shift_agent_initiator_requires_confirmation_then_succeeds_with_valid_proof() {
    let schedule_id = OnCallScheduleId::new();
    let start = chrono::Utc::now();
    let end = start + chrono::Duration::hours(4);

    let mut oncall = MockOnCallService::new();
    oncall
        .expect_create_override()
        .times(1)
        .withf(move |cmd| cmd.schedule_id == schedule_id)
        .return_once(move |cmd| {
            Box::pin(async move {
                Ok(OnCallOverrideDto {
                    id: OnCallOverrideId::new(),
                    schedule_id: cmd.schedule_id,
                    user_id: cmd.user_id,
                    start_at: cmd.start_at,
                    end_at: cmd.end_at,
                    created_by: cmd.actor.user_id,
                })
            })
        });

    let svc = build_service(
        MockCapabilityService::new(),
        MockIncidentService::new(),
        oncall,
    );

    let args = Some(serde_json::json!({
        "schedule_id": schedule_id.to_string(),
        "start_at": start.to_rfc3339(),
        "end_at": end.to_rfc3339(),
    }));
    let a = actor();

    let challenge_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "oncall.take_shift".to_string(),
            args: args.clone(),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("first call (no proof) must return Ok(ConfirmationRequired), not Err");

    assert_eq!(
        challenge_result.status,
        CommandResultStatusDto::ConfirmationRequired
    );
    let challenge = challenge_result
        .confirmation_required
        .expect("ConfirmationRequired must carry a populated challenge");

    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let final_result = svc
        .execute_command(ExecuteCommandCmd {
            actor: a,
            command_id: "oncall.take_shift".to_string(),
            args,
            context: workspace_ctx(),
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("re-submit with valid proof must succeed");

    assert_eq!(final_result.status, CommandResultStatusDto::Success);
}

/// Regression guard: `incident.ack` with a `Palette` initiator (a C1 human
/// surface) stays one-shot — no confirmation challenge, straight to
/// `Success`, matching pre-C2 behaviour exactly.
#[tokio::test]
async fn incident_ack_palette_initiator_is_one_shot_regression_guard() {
    let incident_id = IncidentId::new();
    let now = chrono::Utc::now();

    let mut incident_service = MockIncidentService::new();
    incident_service
        .expect_acknowledge_incident()
        .times(1)
        .withf(move |cmd| cmd.initiator == Some(CommandInitiator::Palette))
        .return_once(move |_| {
            Box::pin(async move { Ok(make_incident_dto(incident_id, Some(now))) })
        });

    let svc = build_service(
        MockCapabilityService::new(),
        incident_service,
        MockOnCallService::new(),
    );

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "incident.ack".to_string(),
            args: Some(serde_json::json!({ "incident_id": incident_id.to_string() })),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("Palette-initiated ack must stay one-shot");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    assert!(result.confirmation_required.is_none());
}

/// Regression guard: `incident.ack` with a `Keybind` initiator (the C1
/// keybind-ack surface) also stays one-shot.
#[tokio::test]
async fn incident_ack_keybind_initiator_is_one_shot_regression_guard() {
    let incident_id = IncidentId::new();
    let now = chrono::Utc::now();

    let mut incident_service = MockIncidentService::new();
    incident_service
        .expect_acknowledge_incident()
        .times(1)
        .withf(move |cmd| cmd.initiator == Some(CommandInitiator::Keybind))
        .return_once(move |_| {
            Box::pin(async move { Ok(make_incident_dto(incident_id, Some(now))) })
        });

    let svc = build_service(
        MockCapabilityService::new(),
        incident_service,
        MockOnCallService::new(),
    );

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "incident.ack".to_string(),
            args: Some(serde_json::json!({ "incident_id": incident_id.to_string() })),
            context: workspace_ctx(),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Keybind,
        })
        .await
        .expect("Keybind-initiated ack must stay one-shot");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    assert!(result.confirmation_required.is_none());
}
