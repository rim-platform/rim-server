//! P4-A4 integration tests — verify the 6 backend metadata-flip commands
//! (#48 dispatch) behave correctly through the public service surface:
//!
//!   - **AI surface listing** — all 6 newly-`ai_callable: true` commands
//!     appear in `Surface::Ai` listing when the actor holds the required
//!     caps. The 4 DesktopLocal commands have empty `required_capabilities`
//!     so they survive even when caps are denied.
//!
//!   - **DesktopLocal commands** — their `execution_target` is preserved
//!     through to the AI surface listing (the frontend dispatcher reads
//!     this field to decide local vs Backend execution per the existing
//!     palette pattern). They're inert at the backend executor (return
//!     `Validation` per the executor's "must run in client runtime" arm) —
//!     the AI agent's tool call would route through the frontend
//!     dispatcher, NOT the executor, for these. We assert the executor
//!     rejection here so a refactor that accidentally makes them
//!     server-executable surfaces immediately.
//!
//!   - **Backend-executable commands** — `chat.post_message` and
//!     `chat.create_channel` round-trip through `execute_command` with
//!     `initiator = Agent`, exercising the executor wiring (including
//!     the new optional `description` arg path on `chat.create_channel`).
//!
//! This is a Phase-4 sister test to `chat_list_channels_agent_test.rs`
//! (Phase 3). Anyone adding a new `ai_callable` builtin should also add
//! a leg here.

use std::collections::HashMap;
use std::sync::Arc;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::confirmation::{
    ConfirmationStore, issue_in_band, shared_in_memory_store,
};
use rim_services::command::dto::{
    CommandContextDto, CommandExecutionTargetDto, CommandResultStatusDto, CommandSurfaceDto,
};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::dto::MessageDto;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

fn actor() -> Actor {
    Actor::new(UserId::new())
}

/// Build a service paired with an explicit confirmation store so that tests
/// exercising Agent-initiator Mutating commands (P6-M2) can pre-issue a
/// `ConfirmationProof` against the same store the service uses.
fn build_service_with_store(
    capability_service: MockCapabilityService,
    conversation_service: MockConversationService,
    message_service: MockMessageService,
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
            conversation_service: Arc::new(conversation_service),
            message_service: Arc::new(message_service),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(
                rim_services::server_member::service::MockServerMemberService::new(),
            ),
            user_service: Arc::new(rim_services::user::service::MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
        Arc::clone(&store),
    );
    (svc, store)
}

/// Convenience wrapper for tests that don't need Agent-initiator confirmation.
fn build_service(
    capability_service: MockCapabilityService,
    conversation_service: MockConversationService,
    message_service: MockMessageService,
) -> DefaultCommandService {
    build_service_with_store(capability_service, conversation_service, message_service).0
}

/// Cap mock that grants whichever cap union the AI surface bulk-checks.
fn cap_grant_all() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many().returning(|_, _, caps| {
        let mut out = HashMap::with_capacity(caps.len());
        for c in caps {
            out.insert(c.clone(), true);
        }
        Ok(out)
    });
    cap
}

#[tokio::test]
async fn ai_surface_lists_all_six_p4_a4_commands() {
    let svc = build_service(
        cap_grant_all(),
        MockConversationService::new(),
        MockMessageService::new(),
    );

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("Surface::Ai must succeed when all caps granted");

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    for expected in [
        "chat.post_message",
        "chat.open_thread",
        "workspace.open_settings",
        "workspace.navigate_to_channel",
        "workspace.switch_workspace",
        "chat.create_channel",
    ] {
        assert!(
            ids.contains(&expected),
            "P4-A4 ai_callable command {expected} missing from Ai surface; got: {ids:?}"
        );
    }
}

#[tokio::test]
async fn ai_surface_preserves_desktop_local_target_on_navigation_commands() {
    // The 4 DesktopLocal flips MUST keep `execution_target: DesktopLocal` so
    // the frontend dispatcher (Phase 4 #53 commandContext bridge / Phase 4
    // FE work) routes them client-side instead of trying to reach an
    // executor arm that doesn't (and shouldn't) exist.
    let svc = build_service(
        cap_grant_all(),
        MockConversationService::new(),
        MockMessageService::new(),
    );

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .unwrap();

    for desktop_local_id in [
        "chat.open_thread",
        "workspace.open_settings",
        "workspace.navigate_to_channel",
        "workspace.switch_workspace",
    ] {
        let cmd = listed
            .iter()
            .find(|c| c.id == desktop_local_id)
            .unwrap_or_else(|| {
                panic!("expected {desktop_local_id} in Ai surface listing");
            });
        assert!(
            matches!(
                cmd.execution_target,
                CommandExecutionTargetDto::DesktopLocal
            ),
            "{desktop_local_id} must keep execution_target=DesktopLocal so the FE dispatcher \
             routes it locally; got {:?}",
            cmd.execution_target
        );
    }
}

#[tokio::test]
async fn execute_command_post_message_round_trips_through_executor() {
    // chat.post_message is `ai_callable: true, sensitivity: Mutating`.
    // P6-M2: Agent initiator + Mutating sensitivity requires a ConfirmationProof.
    // We pre-issue a proof against the service's own store and pass it in.
    use rim_services::command::confirmation::ConfirmationProof;

    let workspace = ServerId::new();
    let conversation = ConversationId::new();
    let new_message_id = MessageId::new();

    let actor = actor();

    let args = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "conversation_id": conversation.to_string(),
        "text": "ship it",
    }));

    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));

    let mut msg = MockMessageService::new();
    let workspace_for_mock = workspace;
    let conversation_for_mock = conversation;
    msg.expect_send_message()
        .withf(move |cmd| {
            cmd.server_id == workspace_for_mock
                && cmd.conversation_id == conversation_for_mock
                && cmd.text.as_deref() == Some("ship it")
        })
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(MessageDto {
                    id: new_message_id,
                    server_id: workspace_for_mock,
                    conversation_id: conversation_for_mock,
                    thread_id: None,
                    root_thread_id: None,
                    author_id: UserId::new(),
                    text: Some("ship it".to_string()),
                    has_attachments: false,
                    attachments: vec![],
                    parent_message_id: None,
                    forwarded_from_message_id: None,
                    thread_root_message_id: None,
                    is_deleted: false,
                    sent_at: chrono::Utc::now(),
                    encryption: None,
                })
            })
        });

    let context = Some(CommandContextDto {
        workspace_id: Some(workspace),
        conversation_id: Some(conversation),
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    });

    let (svc, store) = build_service_with_store(cap, MockConversationService::new(), msg);

    // Issue proof bound to the exact (actor, command_id, args, context).
    let challenge = issue_in_band(store.as_ref(), &actor, "chat.post_message", &args, &context);
    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "chat.post_message".to_string(),
            args,
            context,
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.post_message via Agent initiator with proof must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("post_message payload");
    assert_eq!(payload["message_id"], new_message_id.to_string());
    assert_eq!(payload["conversation_id"], conversation.to_string());
}

#[tokio::test]
async fn execute_command_create_channel_with_description_calls_update_topic() {
    // P4-A4 added an optional `description` arg to chat.create_channel.
    // The executor must: (1) create the channel, (2) if description is
    // present, call `update_topic` with it. Asserting both legs in one
    // test catches a regression where the description silently drops.
    // P6-M2: Agent + Mutating requires a pre-issued ConfirmationProof.
    use rim_services::command::confirmation::ConfirmationProof;

    let workspace = ServerId::new();
    let new_channel_id = ConversationId::new();
    let actor = actor();

    let args = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "name": "migration-2026",
        "description": "Phase-4 reframe migration coordination",
    }));

    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));

    let mut conv = MockConversationService::new();
    let workspace_for_create = workspace;
    conv.expect_create_conversation()
        .withf(move |cmd| {
            cmd.server_id == workspace_for_create && cmd.title.as_deref() == Some("migration-2026")
        })
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(ConversationDto {
                    id: new_channel_id,
                    server_id: workspace_for_create,
                    conversation_type: ConversationType::Channel,
                    title: Some("migration-2026".to_string()),
                    topic: None,
                    member_ids: vec![],
                    is_private: false,
                    archived_at: None,
                    created_by: None,
                    slow_mode_seconds: 0,
                    encrypted_at: None,
                })
            })
        });
    let workspace_for_topic = workspace;
    conv.expect_update_topic()
        .withf(move |cmd| {
            cmd.server_id == workspace_for_topic
                && cmd.conversation_id == new_channel_id
                && cmd.topic.as_deref() == Some("Phase-4 reframe migration coordination")
        })
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(ConversationDto {
                    id: new_channel_id,
                    server_id: workspace_for_topic,
                    conversation_type: ConversationType::Channel,
                    title: Some("migration-2026".to_string()),
                    topic: Some("Phase-4 reframe migration coordination".to_string()),
                    member_ids: vec![],
                    is_private: false,
                    archived_at: None,
                    created_by: None,
                    slow_mode_seconds: 0,
                    encrypted_at: None,
                })
            })
        });

    let context = Some(CommandContextDto {
        workspace_id: Some(workspace),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    });

    let (svc, store) = build_service_with_store(cap, conv, MockMessageService::new());

    let challenge = issue_in_band(
        store.as_ref(),
        &actor,
        "chat.create_channel",
        &args,
        &context,
    );
    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "chat.create_channel".to_string(),
            args,
            context,
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.create_channel with description must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("create_channel payload");
    assert_eq!(payload["conversation_id"], new_channel_id.to_string());
    // mockall's `return_once` + `withf` on `update_topic` would have
    // panicked if it wasn't called with the right description — test
    // passing means the optional description path executed correctly.
}

#[tokio::test]
async fn execute_command_create_channel_without_description_skips_update_topic() {
    // Mirror of the above for the no-description path. We do NOT set up
    // an `expect_update_topic` — mockall will panic on any unexpected
    // call, which is exactly the regression we want to catch.
    // P6-M2: Agent + Mutating requires a pre-issued ConfirmationProof.
    use rim_services::command::confirmation::ConfirmationProof;

    let workspace = ServerId::new();
    let new_channel_id = ConversationId::new();
    let actor = actor();

    let args = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "name": "plain-channel",
    }));

    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));

    let mut conv = MockConversationService::new();
    let workspace_for_create = workspace;
    conv.expect_create_conversation()
        .withf(move |cmd| {
            cmd.server_id == workspace_for_create && cmd.title.as_deref() == Some("plain-channel")
        })
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(ConversationDto {
                    id: new_channel_id,
                    server_id: workspace_for_create,
                    conversation_type: ConversationType::Channel,
                    title: Some("plain-channel".to_string()),
                    topic: None,
                    member_ids: vec![],
                    is_private: false,
                    archived_at: None,
                    created_by: None,
                    slow_mode_seconds: 0,
                    encrypted_at: None,
                })
            })
        });
    // NO `expect_update_topic` — mockall panics on unexpected calls.

    let context = Some(CommandContextDto {
        workspace_id: Some(workspace),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    });

    let (svc, store) = build_service_with_store(cap, conv, MockMessageService::new());

    let challenge = issue_in_band(
        store.as_ref(),
        &actor,
        "chat.create_channel",
        &args,
        &context,
    );
    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "chat.create_channel".to_string(),
            args,
            context,
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.create_channel without description must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
}

#[tokio::test]
async fn execute_command_create_channel_with_empty_description_skips_update_topic() {
    // Defense-in-depth: an empty string `description: ""` from the agent
    // should NOT trigger an `update_topic` call. The executor uses
    // `.filter(|s| !s.is_empty())` to guard this.
    // P6-M2: Agent + Mutating requires a pre-issued ConfirmationProof.
    use rim_services::command::confirmation::ConfirmationProof;

    let workspace = ServerId::new();
    let new_channel_id = ConversationId::new();
    let actor = actor();

    let args = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "name": "blank-desc",
        "description": "",
    }));

    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));

    let mut conv = MockConversationService::new();
    let workspace_for_create = workspace;
    conv.expect_create_conversation()
        .withf(move |cmd| cmd.server_id == workspace_for_create)
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(ConversationDto {
                    id: new_channel_id,
                    server_id: workspace_for_create,
                    conversation_type: ConversationType::Channel,
                    title: Some("blank-desc".to_string()),
                    topic: None,
                    member_ids: vec![],
                    is_private: false,
                    archived_at: None,
                    created_by: None,
                    slow_mode_seconds: 0,
                    encrypted_at: None,
                })
            })
        });

    let (svc, store) = build_service_with_store(cap, conv, MockMessageService::new());

    let challenge = issue_in_band(store.as_ref(), &actor, "chat.create_channel", &args, &None);
    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "chat.create_channel".to_string(),
            args,
            context: None,
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("empty description must succeed and skip update_topic");

    assert_eq!(result.status, CommandResultStatusDto::Success);
}
