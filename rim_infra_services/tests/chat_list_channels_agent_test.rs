//! Integration test — `chat.list_channels` end-to-end through the agent
//! surface.
//!
//! Three legs of the loop are covered here, each one a distinct guarantee
//! the client's AI tools depend on:
//!
//!   1. **Surface filter, positive** — an actor that holds `chat.read` sees
//!      `chat.list_channels` under `Surface::Ai`. (The unit tests in
//!      `command_tests.rs` cover the AI surface listing too; this repeats it
//!      at the integration boundary so a refactor that bypasses
//!      `list_commands` for the AI path doesn't silently widen.)
//!
//!   2. **Surface filter, negative** — an actor WITHOUT `chat.read` does
//!      NOT see `chat.list_channels` under `Surface::Ai`. Locks the
//!      cap-filter dropping side; without this, a regression that ignores
//!      `check_many` results would only show up at `execute_command` time.
//!
//!   3. **execute_command, end-to-end** — `initiator = Agent`, args carry a
//!      real `server_id`, and `MockConversationService::list_channels`
//!      returns two synthetic channels. Asserts the executor projects them
//!      to the agreed wire shape
//!      `{ "channels": [{id,title,conversation_type,is_private,is_archived,is_member}] }`
//!      and **does NOT leak `member_ids`**. The client's AI tools depend on
//!      this contract.
//!
//! That the AI-callable inventory includes `chat.list_channels` is locked at
//! `registry::tests::collect_ai_callable_inventory_matches_reviewed_set`.

use std::collections::HashMap;
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
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto, CommandSurfaceDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

fn actor() -> Actor {
    Actor::new(UserId::new())
}

fn build_service(
    capability_service: MockCapabilityService,
    conversation_service: MockConversationService,
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
            conversation_service: Arc::new(conversation_service),
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

/// Helper — assert the cap union the AI surface bulk-checks:
/// {chat.read, chat.write, admin.manage_members}. `chat.write` comes from the
/// Mutating commands (`chat.post_message`, `chat.create_channel`, ...),
/// `admin.manage_members` from the agent-callable `admin.timeout`. The
/// DesktopLocal commands contribute no caps. Compared as a set so a change
/// in registration order doesn't break the test.
fn caps_match_ai_surface_union(caps: &[String]) -> bool {
    let cap_set: std::collections::HashSet<&str> = caps.iter().map(|s| s.as_str()).collect();
    cap_set.len() == 3
        && cap_set.contains("admin.manage_members")
        && cap_set.contains("chat.read")
        && cap_set.contains("chat.write")
}

/// Leg 1 — actor holding `chat.read` (and `chat.write`) sees
/// `chat.list_channels` in the AI surface listing.
#[tokio::test]
async fn ai_surface_lists_chat_list_channels_when_actor_has_chat_read() {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many()
        .withf(|_actor, _ws, caps| caps_match_ai_surface_union(caps))
        .returning(|_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                out.insert(c.clone(), true);
            }
            Ok(out)
        });

    let svc = build_service(cap, MockConversationService::new());

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("Surface::Ai listing must succeed when cap gate approves");

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    assert!(
        ids.contains(&"chat.list_channels"),
        "expected chat.list_channels in Surface::Ai listing, got: {ids:?}"
    );
}

/// Leg 2 — actor WITHOUT `chat.read` does not see `chat.list_channels`
/// under `Surface::Ai`. Locks the cap-filter drop side; a regression that
/// stopped honouring `check_many=false` would only show up at execute time
/// without this.
///
/// The DesktopLocal commands have empty `required_capabilities`, so when we
/// deny `chat.read` and `chat.write` the cap-gated commands drop but the
/// nav/settings DesktopLocal commands survive; `chat.list_channels` must be
/// dropped.
#[tokio::test]
async fn ai_surface_omits_chat_list_channels_when_actor_lacks_chat_read() {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many()
        .withf(|_actor, _ws, caps| caps_match_ai_surface_union(caps))
        .returning(|_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                // Deny every cap requested — `chat.read` and `chat.write`
                // both withheld.
                out.insert(c.clone(), false);
            }
            Ok(out)
        });

    let svc = build_service(cap, MockConversationService::new());

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("Surface::Ai listing must succeed even when no commands pass");

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    assert!(
        !ids.contains(&"chat.list_channels"),
        "chat.list_channels MUST be filtered out when actor lacks chat.read; got: {ids:?}"
    );
    assert!(
        !ids.contains(&"chat.post_message"),
        "chat.post_message MUST be filtered out when actor lacks chat.write; got: {ids:?}"
    );
    assert!(
        !ids.contains(&"chat.create_channel"),
        "chat.create_channel MUST be filtered out when actor lacks chat.write; got: {ids:?}"
    );
    // The 4 DesktopLocal navigation/settings commands don't require any cap,
    // so they survive cap denial. They're inert from the backend's
    // perspective — frontend dispatcher routes them client-side.
    let surviving_no_cap_commands = [
        "chat.open_thread",
        "workspace.open_settings",
        "workspace.navigate_to_channel",
        "workspace.switch_workspace",
    ];
    for expected in surviving_no_cap_commands {
        assert!(
            ids.contains(&expected),
            "{expected} has empty required_capabilities and must survive cap denial; got: {ids:?}"
        );
    }
}

/// Leg 3 — `execute_command` with `initiator = Agent` reaches the executor
/// arm, calls `ConversationService::list_channels`, and projects to the
/// agreed wire shape. Crucially, asserts `member_ids` is **NOT** in the
/// payload (no PII, same rule as executor/member.rs::list_members).
#[tokio::test]
async fn execute_command_chat_list_channels_returns_projected_payload() {
    let workspace = ServerId::new();

    // Cap gate: actor holds chat.read for this workspace.
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.read")
        .returning(|_, _, _| Ok(()));

    // Conversation service: return two synthetic channels with member_ids
    // populated, so the projection test verifies they're stripped.
    let workspace_for_mock = workspace;
    let channel_a_id = ConversationId::new();
    let channel_b_id = ConversationId::new();
    let secret_member = UserId::new();
    // The caller is a member of `general` only; `random` is a private, archived channel
    // the mock returns regardless of the default filter.
    let caller = actor();

    let mut conv = MockConversationService::new();
    conv.expect_list_channels()
        .withf(move |q| q.server_id == workspace_for_mock)
        .return_once(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![
                        ConversationDto {
                            id: channel_a_id,
                            server_id: workspace_for_mock,
                            conversation_type: ConversationType::Channel,
                            title: Some("general".to_string()),
                            topic: None,
                            member_ids: vec![secret_member, caller.user_id],
                            is_private: false,
                            archived_at: None,
                            created_by: None,
                            slow_mode_seconds: 0,
                            encrypted_at: None,
                        },
                        ConversationDto {
                            id: channel_b_id,
                            server_id: workspace_for_mock,
                            conversation_type: ConversationType::Channel,
                            title: Some("random".to_string()),
                            topic: None,
                            member_ids: vec![secret_member],
                            is_private: true,
                            archived_at: Some(chrono::Utc::now()),
                            created_by: None,
                            slow_mode_seconds: 0,
                            encrypted_at: None,
                        },
                    ],
                    None,
                    None,
                ))
            })
        });

    let svc = build_service(cap, conv);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: caller,
            command_id: "chat.list_channels".to_string(),
            args: Some(serde_json::json!({
                "server_id": workspace.to_string(),
            })),
            context: Some(CommandContextDto {
                workspace_id: Some(workspace),
                conversation_id: None,
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            }),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("execute_command for chat.list_channels must succeed under valid actor");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    assert_eq!(result.message, "2 channels found");

    let payload = result.payload.expect("payload must be present");
    let channels = payload
        .get("channels")
        .and_then(|v| v.as_array())
        .expect("payload must have a `channels` array");
    assert_eq!(channels.len(), 2);

    // Wire-shape assertion: each channel projects to {id, title,
    // conversation_type, is_private, is_archived, is_member} only — member_ids
    // NOT present (the caller's membership is derived server-side).
    for (channel, expected_id, expected_title, private, archived, member) in [
        (&channels[0], channel_a_id, "general", false, false, true),
        (&channels[1], channel_b_id, "random", true, true, false),
    ] {
        let mut keys: Vec<&str> = channel
            .as_object()
            .expect("channel object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "conversation_type",
                "id",
                "is_archived",
                "is_member",
                "is_private",
                "title"
            ]
        );
        assert_eq!(channel["is_private"], private);
        assert_eq!(channel["is_archived"], archived);
        assert_eq!(channel["is_member"], member);
        assert_eq!(
            channel.get("id").and_then(|v| v.as_str()),
            Some(expected_id.to_string().as_str()),
        );
        assert_eq!(
            channel.get("title").and_then(|v| v.as_str()),
            Some(expected_title),
        );
        // Pin the wire string against `ConversationType::as_str()` rather
        // than the Debug repr. If a variant gets renamed
        // (e.g. `Channel` → `PublicChannel`), the contract break surfaces
        // in `as_str()` review, not as a silent agent payload drift.
        assert_eq!(
            channel.get("conversation_type").and_then(|v| v.as_str()),
            Some(rim_core::conversation::model::ConversationType::Channel.as_str()),
        );
        // PII (member_ids) MUST NOT leak through this tool's payload. If the
        // projection passes `c` directly through serde, this assertion
        // catches it.
        assert!(
            channel.get("member_ids").is_none(),
            "member_ids must NOT appear in the chat.list_channels payload \
             — PII leak; got channel: {channel}",
        );
    }
}
