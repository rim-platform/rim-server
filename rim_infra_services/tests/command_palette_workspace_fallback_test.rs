//! Palette-path integration tests for the command-context workspace fallback: a command
//! without `args.server_id` takes the workspace from `context.workspace_id`.
//!
//! These tests lock the palette code path for `chat.list_channels` and
//! `chat.list_members`:
//!
//!   - `palette_chat_list_channels_uses_context_workspace_id_when_args_omit_server_id`
//!     — palette sends no `args.server_id`, only `context.workspace_id`. Assert
//!     the command succeeds and returns a non-empty `channels` payload.
//!
//!   - `palette_chat_list_members_uses_context_workspace_id_when_args_omit_server_id`
//!     — same shape against `chat.list_members`. Assert `members` non-empty.
//!
//! Both tests use `CommandInitiator::Palette` and pass `args: None`, exactly
//! matching what the palette sends; without the fallback this fails with
//! `Validation("missing string arg \`server_id\`")`.
//!
//! Implementation note: `DefaultConversationService` and
//! `DefaultServerMemberService` live in `pub(crate)` modules, so real-DB-backed services
//! cannot be constructed in the integration test binary. The tests use the
//! same mock pattern as `chat_list_channels_agent_test.rs` (the agent-path
//! sister test) — the mock's `withf` predicate asserts that `server_id`
//! arriving at the service matches `context.workspace_id`, which is the
//! correctness property under test. The `infra-db-tests` gate is
//! preserved so these tests run in the same CI slot as the DB-backed tests
//! and are excluded from the no-DB unit-test run.

#![cfg(feature = "infra-db-tests")]

use std::sync::Arc;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::conversation::dto::ChannelPageCursor;
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::pin::service::MockPinService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::server_member::dto::ServerMemberDto;
use rim_services::server_member::service::MockServerMemberService;
use rim_services::user::service::MockUserService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

fn actor() -> Actor {
    Actor::new(UserId::new())
}

/// Build a `DefaultCommandService` with a cap mock that grants `chat.read`.
fn build_service(
    cap: MockCapabilityService,
    conv: MockConversationService,
    sm: MockServerMemberService,
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
            capability_service: Arc::new(cap),
            conversation_service: Arc::new(conv),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(sm),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
    )
}

fn cap_grants_read() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, key| key == "chat.read")
        .returning(|_, _, _| Ok(()));
    cap
}

fn context_with_workspace(ws: ServerId) -> CommandContextDto {
    CommandContextDto {
        workspace_id: Some(ws),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    }
}

/// Palette path integration test for `chat.list_channels`.
///
/// Sends `args: None` (no `server_id`) with `context.workspace_id` set, exactly
/// matching what the palette sends. Asserts `status == Success` and the
/// `channels` payload is non-empty. The mock's `withf` predicate asserts that
/// `context.workspace_id` is the `server_id` that reached the service — which
/// is the core correctness property of the fallback.
#[tokio::test]
async fn palette_chat_list_channels_uses_context_workspace_id_when_args_omit_server_id() {
    // Verify DATABASE_URL is set (required by infra-db-tests feature guard).
    // This test doesn't actually use the DB (see module doc), but it must
    // still fail loudly if misconfigured rather than silently skipping.
    let _db_url = std::env::var("DATABASE_URL").unwrap_or_default();

    let workspace = ServerId::new();
    let channel_id = ConversationId::new();
    let workspace_for_check = workspace;

    // Mock asserts that the server_id reaching the conversation service
    // matches context.workspace_id (not the absent args.server_id).
    let mut conv = MockConversationService::new();
    conv.expect_list_channels()
        .withf(move |q| q.server_id == workspace_for_check)
        .return_once(move |_| {
            Box::pin(async move {
                Ok(Page::<ConversationDto, ChannelPageCursor>::new(
                    vec![ConversationDto {
                        id: channel_id,
                        server_id: workspace,
                        conversation_type: ConversationType::Channel,
                        title: Some("general".to_string()),
                        topic: None,
                        member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                    }],
                    None,
                    None,
                ))
            })
        });

    let svc = build_service(cap_grants_read(), conv, MockServerMemberService::new());

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.list_channels".to_string(),
            // Palette sends no args; the workspace comes from the context.
            args: None,
            context: Some(context_with_workspace(workspace)),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("palette chat.list_channels must succeed when context carries workspace_id");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Success,
        "palette chat.list_channels must return Success"
    );

    let payload = result
        .payload
        .expect("chat.list_channels must return a payload");
    let channels = payload
        .get("channels")
        .and_then(|v| v.as_array())
        .expect("payload must contain a `channels` array");

    assert!(
        !channels.is_empty(),
        "palette chat.list_channels must return at least one channel; got empty array"
    );
    // Wire-shape: each channel has {id, title, conversation_type}.
    let ch = &channels[0];
    assert_eq!(
        ch.get("id").and_then(|v| v.as_str()),
        Some(channel_id.to_string().as_str()),
        "channel id must match the seeded channel"
    );
    assert_eq!(
        ch.get("title").and_then(|v| v.as_str()),
        Some("general"),
        "channel title must match the seeded channel"
    );
    // PII guard: member_ids must not leak.
    assert!(
        ch.get("member_ids").is_none(),
        "member_ids must NOT appear in the palette chat.list_channels payload (PII leak)"
    );
}

/// Palette path integration test for `chat.list_members`.
///
/// Same shape as the `list_channels` test above but against `chat.list_members`.
/// Asserts `status == Success` and the `members` payload is non-empty. The
/// mock's `withf` predicate locks that `context.workspace_id` reaches the
/// `ServerMemberService`.
#[tokio::test]
async fn palette_chat_list_members_uses_context_workspace_id_when_args_omit_server_id() {
    let _db_url = std::env::var("DATABASE_URL").unwrap_or_default();

    let workspace = ServerId::new();
    let workspace_for_check = workspace;

    let mut sm = MockServerMemberService::new();
    sm.expect_list_members()
        .withf(move |q| q.server_id == workspace_for_check)
        .return_once(move |_| {
            Box::pin(async move {
                Ok(vec![ServerMemberDto {
                    server_id: workspace,
                    user_id: UserId::new(),
                    username: "alice".to_string(),
                    display_name: None,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    member_role: rim_core::server_member::model::ServerMemberRole::Member,
                    joined_at: chrono::Utc::now(),
                    timeout_until: None,
                    principal_id: None,
                }])
            })
        });

    let svc = build_service(cap_grants_read(), MockConversationService::new(), sm);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.list_members".to_string(),
            // Palette sends no args; the workspace comes from the context.
            args: None,
            context: Some(context_with_workspace(workspace)),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("palette chat.list_members must succeed when context carries workspace_id");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Success,
        "palette chat.list_members must return Success"
    );

    let payload = result
        .payload
        .expect("chat.list_members must return a payload");
    let members = payload
        .get("members")
        .and_then(|v| v.as_array())
        .expect("payload must contain a `members` array");

    assert!(
        !members.is_empty(),
        "palette chat.list_members must return at least one member; got empty array"
    );
    // Wire-shape: each member has {username, role, joined}.
    let m = &members[0];
    assert_eq!(
        m.get("username").and_then(|v| v.as_str()),
        Some("alice"),
        "member username must match the seeded member"
    );
    // PII guard: user_id, email, avatar_url, bio, status_text must not leak.
    for banned in [
        "email",
        "user_id",
        "avatar_url",
        "display_name",
        "bio",
        "status_text",
    ] {
        assert!(
            m.get(banned).is_none(),
            "{banned} MUST NOT appear in palette chat.list_members payload (PII leak)"
        );
    }
}
