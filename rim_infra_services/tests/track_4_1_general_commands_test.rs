//! Track 4.1 integration tests: the general chat, navigation and workspace-state builtins are
//! AI-callable, and the agent reaches them through the same registry and confirmation gate as
//! the incident commands.
//!
//!   - **AI surface listing**: every Track 4.1 flip appears on `Surface::Ai`, desktop-local
//!     navigation keeps `execution_target: DesktopLocal` so the client runs it locally.
//!   - **Confirmation gate**: read-only flips run directly for the agent; the ones that write
//!     (`chat.mark_as_read`, `chat.create_group`, `state.set`, `state.incr`, `state.delete`) are
//!     `Mutating`, so an `Agent` call without a proof gets `CONFIRMATION_REQUIRED` and the
//!     service is never reached, while human initiators still run them in one step.

use std::collections::HashMap;
use std::sync::Arc;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_state::model::WorkspaceStateId;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::confirmation::{
    ConfirmationProof, ConfirmationStore, issue_in_band, shared_in_memory_store,
};
use rim_services::command::dto::{
    CommandContextDto, CommandExecutionTargetDto, CommandResultStatusDto, CommandSurfaceDto,
};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::dto::{WorkspaceStateDto, WorkspaceStateScopeDto};
use rim_services::workspace_state::service::MockWorkspaceStateService;

/// Desktop-local navigation flipped in Track 4.1; the client runs these, never the server.
const DESKTOP_LOCAL_FLIPS: [&str; 11] = [
    "workspace.open_search",
    "workspace.search_in_channel",
    "workspace.open_marketplace",
    "workspace.toggle_sidebar",
    "workspace.previous_channel",
    "workspace.next_channel",
    "workspace.next_unread_channel",
    "workspace.close_panel",
    "workspace.reopen_last_thread",
    "chat.scroll_to_bottom",
    "chat.scroll_to_top",
];

/// Backend flips that only read: the agent runs them without a confirmation.
const READ_ONLY_BACKEND_FLIPS: [&str; 2] = ["state.get", "state.list"];

/// Backend flips that write: the agent needs a confirmation proof for each.
const MUTATING_BACKEND_FLIPS: [&str; 5] = [
    "chat.mark_as_read",
    "chat.create_group",
    "state.set",
    "state.incr",
    "state.delete",
];

fn build_service(
    capability_service: MockCapabilityService,
    workspace_state_service: MockWorkspaceStateService,
) -> (DefaultCommandService, Arc<dyn ConfirmationStore>) {
    let mut user_repo = MockUserCommandRepository::new();
    user_repo
        .expect_list_for_user()
        .returning(|_| Box::pin(async { Ok(vec![]) }));

    let store = shared_in_memory_store();
    let svc = DefaultCommandService::with_confirmation_store(
        CommandRepos {
            user_command_repo: Arc::new(user_repo),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(capability_service),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(
                rim_services::server_member::service::MockServerMemberService::new(),
            ),
            user_service: Arc::new(rim_services::user::service::MockUserService::new()),
            workspace_state_service: Arc::new(workspace_state_service),
            audit_log_service: None,
            incident_service: Arc::new(rim_services::incident::service::MockIncidentService::new()),
            oncall_service: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
            workspace_feature_service: Arc::new(MockWorkspaceFeatureService::all_enabled()),
            moderation_service: Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
        Arc::clone(&store),
    );
    (svc, store)
}

fn cap_grant_all() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many().returning(|_, _, caps| {
        let mut out = HashMap::with_capacity(caps.len());
        for c in caps {
            out.insert(c.clone(), true);
        }
        Ok(out)
    });
    cap.expect_check().returning(|_, _, _| Ok(()));
    cap
}

fn context(workspace: ServerId) -> Option<CommandContextDto> {
    Some(CommandContextDto {
        workspace_id: Some(workspace),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    })
}

fn state_entry(workspace: ServerId, key: &str, value: serde_json::Value) -> WorkspaceStateDto {
    let now = chrono::Utc::now();
    WorkspaceStateDto {
        id: WorkspaceStateId::new(),
        server_id: workspace,
        key: key.to_string(),
        value,
        scope: WorkspaceStateScopeDto::Server,
        user_id: None,
        version: 1,
        expires_at: None,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn ai_surface_lists_every_track_4_1_command() {
    let (svc, _) = build_service(cap_grant_all(), MockWorkspaceStateService::new());

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: Actor::new(UserId::new()),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("Surface::Ai must succeed when all caps granted");

    for id in DESKTOP_LOCAL_FLIPS {
        let cmd = listed
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id} missing from the Ai surface"));
        assert_eq!(
            cmd.execution_target,
            CommandExecutionTargetDto::DesktopLocal,
            "{id} must stay desktop-local so the client runs it"
        );
        assert!(
            !cmd.requires_confirmation_for_initiator(CommandInitiator::Agent),
            "{id} only moves the user's view and must run without a confirmation"
        );
    }
    for id in READ_ONLY_BACKEND_FLIPS
        .iter()
        .chain(MUTATING_BACKEND_FLIPS.iter())
    {
        let cmd = listed
            .iter()
            .find(|c| c.id == *id)
            .unwrap_or_else(|| panic!("{id} missing from the Ai surface"));
        assert_eq!(cmd.execution_target, CommandExecutionTargetDto::Backend);
        assert!(
            cmd.nl_description.is_some(),
            "{id} needs an nl_description for the agent"
        );
    }
}

#[tokio::test]
async fn confirmation_gate_matches_read_and_write_split() {
    let (svc, _) = build_service(cap_grant_all(), MockWorkspaceStateService::new());
    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: Actor::new(UserId::new()),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .unwrap();
    let find = |id: &str| {
        listed
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id} missing from the Ai surface"))
    };

    for id in READ_ONLY_BACKEND_FLIPS {
        assert!(
            !find(id).requires_confirmation_for_initiator(CommandInitiator::Agent),
            "{id} only reads and must run directly for the agent"
        );
    }
    for id in MUTATING_BACKEND_FLIPS {
        let cmd = find(id);
        assert!(
            cmd.requires_confirmation_for_initiator(CommandInitiator::Agent),
            "{id} writes and must be confirmation-gated for the agent"
        );
        assert!(
            !cmd.requires_confirmation_for_initiator(CommandInitiator::Palette),
            "{id} must stay one step for a human in the palette"
        );
    }
}

#[tokio::test]
async fn agent_state_get_runs_without_confirmation() {
    let workspace = ServerId::new();
    let mut state = MockWorkspaceStateService::new();
    state
        .expect_get()
        .withf(move |q| q.server_id == workspace && q.key == "deploy.lock")
        .times(1)
        .returning(move |_| {
            Box::pin(async move {
                Ok(Some(state_entry(
                    workspace,
                    "deploy.lock",
                    serde_json::json!(true),
                )))
            })
        });
    let (svc, _) = build_service(cap_grant_all(), state);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(UserId::new()),
            command_id: "state.get".to_string(),
            args: Some(serde_json::json!({ "key": "deploy.lock" })),
            context: context(workspace),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("state.get must run for the agent without a proof");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    assert_eq!(result.payload.expect("state payload")["value"], true);
}

#[tokio::test]
async fn agent_state_set_without_proof_returns_challenge_and_does_not_write() {
    let workspace = ServerId::new();
    let mut state = MockWorkspaceStateService::new();
    state.expect_set().never();
    let (svc, _) = build_service(cap_grant_all(), state);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(UserId::new()),
            command_id: "state.set".to_string(),
            args: Some(serde_json::json!({ "key": "deploy.lock", "value": "true" })),
            context: context(workspace),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("an unconfirmed agent write returns a challenge, not an error");

    assert_eq!(result.status, CommandResultStatusDto::ConfirmationRequired);
    assert!(result.confirmation_required.is_some());
}

#[tokio::test]
async fn agent_state_set_with_proof_writes() {
    let workspace = ServerId::new();
    let actor = Actor::new(UserId::new());
    let mut state = MockWorkspaceStateService::new();
    state
        .expect_set()
        .withf(move |cmd| {
            cmd.server_id == workspace
                && cmd.key == "deploy.lock"
                && cmd.value == serde_json::json!(true)
        })
        .times(1)
        .returning(move |cmd| {
            Box::pin(async move { Ok(state_entry(workspace, &cmd.key, cmd.value)) })
        });
    let (svc, store) = build_service(cap_grant_all(), state);

    let args = Some(serde_json::json!({ "key": "deploy.lock", "value": "true" }));
    let ctx = context(workspace);
    let challenge = issue_in_band(store.as_ref(), &actor, "state.set", &args, &ctx);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "state.set".to_string(),
            args,
            context: ctx,
            confirmation: Some(ConfirmationProof::InBandToken {
                token: challenge.token,
                args_hash: challenge.args_hash,
            }),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("state.set with a valid proof must run for the agent");

    assert_eq!(result.status, CommandResultStatusDto::Success);
}

#[tokio::test]
async fn palette_state_set_still_runs_in_one_step() {
    let workspace = ServerId::new();
    let mut state = MockWorkspaceStateService::new();
    state.expect_set().times(1).returning(move |cmd| {
        Box::pin(async move { Ok(state_entry(workspace, &cmd.key, cmd.value)) })
    });
    let (svc, _) = build_service(cap_grant_all(), state);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(UserId::new()),
            command_id: "state.set".to_string(),
            args: Some(serde_json::json!({ "key": "theme", "value": "dark" })),
            context: context(workspace),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("a human state.set needs no confirmation");

    assert_eq!(result.status, CommandResultStatusDto::Success);
}
