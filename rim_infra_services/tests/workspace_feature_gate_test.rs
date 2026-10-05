//! Track 0.4 — the per-workspace feature gate on the command layer
//! (`docs/specs/track-0-4-optional-incident-module.md`).
//!
//! The four incident/on-call builtins (`incident.ack`, `incident.escalate`, `oncall.who`,
//! `oncall.take_shift`) carry `feature: Some(WorkspaceFeature::Incidents)`. These tests pin:
//! listing hides them when the feature is off or there is no workspace context, execution
//! rejects them with `FeatureDisabled`, `command_discover` inherits the filter, and nested
//! dispatch (user-command steps, pipe stages) hits the same gate. Macros execute every step
//! through the public `execute_command`; one test drives a real `DefaultMacroService` over the
//! real command service to pin that a gated step fails the macro.

use std::sync::Arc;

use rim_core::command::model::{UserCommand, UserCommandId};
use rim_core::command::repository::MockUserCommandRepository;
use rim_core::macro_def::model::{MacroDef, MacroId};
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::workspace_feature::model::WorkspaceFeature;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_infra_services::macro_def::{DefaultMacroService, MacroRepos, MacroServices};
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto, CommandSurfaceDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::incident::service::MockIncidentService;
use rim_services::macro_def::request::ExecuteMacroCmd;
use rim_services::macro_def::service::MacroService;
use rim_services::message::service::MockMessageService;
use rim_services::message_reaction::service::MockMessageReactionService;
use rim_services::oncall::service::MockOnCallService;
use rim_services::pin::service::MockPinService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::server_member::service::MockServerMemberService;
use rim_services::user::service::MockUserService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;
use rim_services::{Actor, ServiceError};

const GATED: [&str; 4] = [
    "incident.ack",
    "incident.escalate",
    "oncall.who",
    "oncall.take_shift",
];

fn workspace_ctx(server_id: ServerId) -> Option<CommandContextDto> {
    Some(CommandContextDto {
        workspace_id: Some(server_id),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    })
}

fn allow_all_caps() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many()
        .returning(|_, _, caps| Ok(caps.iter().map(|c| (c.clone(), true)).collect()));
    cap.expect_check().returning(|_, _, _| Ok(()));
    cap
}

/// The incident/on-call services are bare mocks with no expectations: any dispatch that slips
/// past the gate panics the test.
fn build_service(
    user_repo: MockUserCommandRepository,
    features: MockWorkspaceFeatureService,
) -> DefaultCommandService {
    DefaultCommandService::new(
        CommandRepos {
            user_command_repo: Arc::new(user_repo),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(allow_all_caps()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(MockMessageReactionService::new()),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: Arc::new(MockIncidentService::new()),
            oncall_service: Arc::new(MockOnCallService::new()),
            workspace_feature_service: Arc::new(features),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
        },
    )
}

fn empty_user_repo() -> MockUserCommandRepository {
    let mut repo = MockUserCommandRepository::new();
    repo.expect_list_for_user()
        .returning(|_| Box::pin(async { Ok(vec![]) }));
    repo.expect_get_by_key_for_user()
        .returning(|_, _| Box::pin(async { Ok(None) }));
    repo
}

fn ids(commands: &[rim_services::command::dto::CommandDefinitionDto]) -> Vec<&str> {
    commands.iter().map(|c| c.id.as_str()).collect()
}

fn ack_cmd(context: Option<CommandContextDto>) -> ExecuteCommandCmd {
    ExecuteCommandCmd {
        actor: Actor::new(UserId::new()),
        command_id: "incident.ack".to_string(),
        args: Some(serde_json::json!({ "incident_id": uuid::Uuid::now_v7().to_string() })),
        context,
        confirmation: None,
        is_pipe: false,
        initiator: CommandInitiator::Palette,
    }
}

fn assert_feature_disabled(err: ServiceError) {
    match err {
        ServiceError::FeatureDisabled { feature } => assert_eq!(feature, "incidents"),
        other => panic!("expected FeatureDisabled, got {other:?}"),
    }
}

#[tokio::test]
async fn palette_hides_gated_commands_when_feature_disabled() {
    let svc = build_service(
        empty_user_repo(),
        MockWorkspaceFeatureService::all_disabled(),
    );
    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: Actor::new(UserId::new()),
            context: workspace_ctx(ServerId::new()),
            surface: CommandSurfaceDto::Palette,
        })
        .await
        .unwrap();
    let listed = ids(&listed);
    for id in GATED {
        assert!(!listed.contains(&id), "{id} must be hidden; got {listed:?}");
    }
    assert!(listed.contains(&"chat.post_message"));
}

#[tokio::test]
async fn palette_shows_gated_commands_when_feature_enabled_with_one_lookup() {
    let server_id = ServerId::new();
    let mut features = MockWorkspaceFeatureService::new();
    features
        .expect_enabled_features()
        .withf(move |s| *s == server_id)
        .times(1)
        .returning(|_| Box::pin(async { Ok(vec![WorkspaceFeature::Incidents]) }));

    let svc = build_service(empty_user_repo(), features);
    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: Actor::new(UserId::new()),
            context: workspace_ctx(server_id),
            surface: CommandSurfaceDto::Palette,
        })
        .await
        .unwrap();
    let listed = ids(&listed);
    for id in ["incident.ack", "incident.escalate", "oncall.take_shift"] {
        assert!(listed.contains(&id), "{id} must be listed; got {listed:?}");
    }
}

#[tokio::test]
async fn gated_commands_hidden_without_workspace_context() {
    // No expectations: the feature service must not be consulted without a workspace.
    let svc = build_service(empty_user_repo(), MockWorkspaceFeatureService::new());
    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: Actor::new(UserId::new()),
            context: None,
            surface: CommandSurfaceDto::Palette,
        })
        .await
        .unwrap();
    let listed = ids(&listed);
    for id in GATED {
        assert!(!listed.contains(&id), "{id} must be hidden; got {listed:?}");
    }
}

#[tokio::test]
async fn command_discover_omits_gated_commands_when_feature_disabled() {
    let svc = build_service(
        empty_user_repo(),
        MockWorkspaceFeatureService::all_disabled(),
    );
    let found = svc
        .command_discover(
            &Actor::new(UserId::new()),
            Some("acknowledge incident".to_string()),
            Some(100),
            Some(ServerId::new()),
        )
        .await
        .unwrap();
    let found = ids(&found);
    for id in GATED {
        assert!(
            !found.contains(&id),
            "{id} must not be discoverable; got {found:?}"
        );
    }
}

#[tokio::test]
async fn command_discover_includes_gated_commands_when_feature_enabled() {
    let svc = build_service(
        empty_user_repo(),
        MockWorkspaceFeatureService::all_enabled(),
    );
    let found = svc
        .command_discover(
            &Actor::new(UserId::new()),
            None,
            Some(100),
            Some(ServerId::new()),
        )
        .await
        .unwrap();
    let found = ids(&found);
    for id in GATED {
        assert!(
            found.contains(&id),
            "{id} must be discoverable; got {found:?}"
        );
    }
}

#[tokio::test]
async fn execute_rejects_gated_command_when_feature_disabled() {
    let svc = build_service(
        empty_user_repo(),
        MockWorkspaceFeatureService::all_disabled(),
    );
    let err = svc
        .execute_command(ack_cmd(workspace_ctx(ServerId::new())))
        .await
        .unwrap_err();
    assert_feature_disabled(err);
}

#[tokio::test]
async fn execute_rejects_gated_command_without_workspace_context() {
    let svc = build_service(empty_user_repo(), MockWorkspaceFeatureService::new());
    let err = svc.execute_command(ack_cmd(None)).await.unwrap_err();
    assert_feature_disabled(err);
}

#[tokio::test]
async fn user_command_step_on_gated_command_is_rejected() {
    let owner = UserId::new();
    let user_command = UserCommand {
        id: UserCommandId::new(),
        owner_user_id: owner,
        command_key: "user.ack_it".to_string(),
        title: "Ack it".to_string(),
        description: String::new(),
        steps_json: serde_json::json!([{
            "command_id": "incident.ack",
            "args": { "incident_id": uuid::Uuid::now_v7().to_string() }
        }]),
        is_enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let mut repo = MockUserCommandRepository::new();
    repo.expect_get_by_key_for_user().returning(move |_, key| {
        let found = (key == "user.ack_it").then(|| user_command.clone());
        Box::pin(async move { Ok(found) })
    });

    let svc = build_service(repo, MockWorkspaceFeatureService::all_disabled());
    let err = svc
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(owner),
            command_id: "user.ack_it".to_string(),
            args: None,
            context: workspace_ctx(ServerId::new()),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await
        .unwrap_err();
    assert_feature_disabled(err);
}

#[tokio::test]
async fn pipe_stage_on_gated_command_fails_with_feature_disabled() {
    let svc = build_service(
        empty_user_repo(),
        MockWorkspaceFeatureService::all_disabled(),
    );
    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(UserId::new()),
            command_id: format!("oncall.who schedule_id={}", uuid::Uuid::now_v7()),
            args: None,
            context: workspace_ctx(ServerId::new()),
            confirmation: None,
            is_pipe: true,
            initiator: CommandInitiator::Palette,
        })
        .await
        .unwrap();
    assert_eq!(result.status, CommandResultStatusDto::Error);
    assert!(
        result
            .message
            .contains("workspace feature `incidents` is disabled"),
        "unexpected pipe error message: {}",
        result.message
    );
}

#[tokio::test]
async fn macro_step_on_gated_command_fails_the_macro() {
    let owner = UserId::new();
    let server_id = ServerId::new();
    let macro_id = MacroId::new();
    let macro_def = MacroDef {
        id: macro_id,
        server_id,
        user_id: owner,
        name: "ack-and-go".to_string(),
        description: None,
        steps_json: serde_json::json!([{
            "commandId": "incident.ack",
            "args": { "incident_id": uuid::Uuid::now_v7().to_string() }
        }]),
        ai_callable: false,
        nl_description: String::new(),
        intent_samples: vec![],
        sensitivity: "safe".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let mut macro_repo = MockMacroRepository::new();
    macro_repo.expect_get_by_id().returning(move |_| {
        let found = macro_def.clone();
        Box::pin(async move { Ok(Some(found)) })
    });
    let mut member_repo = MockServerMemberRepository::new();
    member_repo.expect_is_member().returning(|_, _| Ok(true));

    // Real command service; the incident service mock has no expectations, so a step that
    // slipped past the feature gate would panic the test. The gate must be consulted for the
    // step's workspace.
    let mut features = MockWorkspaceFeatureService::new();
    features
        .expect_is_enabled()
        .withf(move |s, f| *s == server_id && *f == WorkspaceFeature::Incidents)
        .times(1..)
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let command_service = build_service(empty_user_repo(), features);
    let macros = DefaultMacroService::new(
        MacroRepos {
            macro_repo: Arc::new(macro_repo),
            server_member_repo: Arc::new(member_repo),
        },
        MacroServices {
            command_service: Arc::new(command_service),
            audit_log_service: None,
            confirmation_store: None,
        },
    );

    let result = macros
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(owner),
            macro_id,
            command_context: workspace_ctx(server_id),
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .unwrap();
    assert_eq!(result.status, CommandResultStatusDto::Error);
    assert!(
        result.message.contains("failed on a step"),
        "unexpected macro message: {}",
        result.message
    );
}
