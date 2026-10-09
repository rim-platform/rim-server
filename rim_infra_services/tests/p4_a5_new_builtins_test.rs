//! P4-A5 integration tests — the five new AI-callable backend builtins
//! (#49 dispatch). Covers, per command:
//!
//!   - AI surface listing: command appears when the actor holds the
//!     required cap, omitted when they don't.
//!   - `execute_command` round-trip with `initiator = Agent`: the
//!     executor calls the right service method and the payload matches
//!     the documented wire shape.
//!   - **PII no-leak** on `chat.list_members`: `email`, `user_id`,
//!     `avatar_url`, `display_name`, `bio`, `status_text` MUST NOT appear
//!     in the projected payload. Matches the `member_ids`-drop pattern
//!     from `chat_list_channels_agent_test.rs`.
//!   - **DataLoader confirmation** on `search_messages` /
//!     `get_recent_messages`: when the result set spans N authors, the
//!     executor must call `UserService::get_users_by_ids` exactly once
//!     regardless of N (Widow no-go against N+1). Verified via mockall's
//!     single-expectation semantics — a second call would panic.
//!   - **Toggle round-trip** on `chat.toggle_reaction`: two successive
//!     toggles on the same `(message, emoji, actor)` flip `action` from
//!     `"added"` to `"removed"`. Locks the bidirectional semantics so a
//!     future "add-only" refactor can't silently regress.
//!
//! Sister to `p4_a4_metadata_flips_test.rs`; anyone adding a new
//! `ai_callable` builtin also updates the "surface listing" legs in
//! both files.

use std::collections::HashMap;
use std::sync::Arc;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::message::model::MessageId;
use rim_core::repository::pagination::Page;
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
use rim_services::command::dto::{CommandResultStatusDto, CommandSurfaceDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::{ExecuteCommandCmd, ListCommandsQuery};
use rim_services::command::service::CommandService;
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::dto::{MessageDto, MessageSearchHitDto};
use rim_services::message::service::MockMessageService;
use rim_services::message_reaction::dto::ReactionGroupDto;
use rim_services::message_reaction::service::MockMessageReactionService;
use rim_services::pin::service::MockPinService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::server_member::dto::ServerMemberDto;
use rim_services::server_member::service::MockServerMemberService;
use rim_services::user::dto::UserDto;
use rim_services::user::service::MockUserService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

fn actor_with_id(user_id: UserId) -> Actor {
    Actor::new(user_id)
}

fn actor() -> Actor {
    actor_with_id(UserId::new())
}

struct Mocks {
    capability: MockCapabilityService,
    conversation: MockConversationService,
    message: MockMessageService,
    message_reaction: MockMessageReactionService,
    pin: MockPinService,
    server_member: MockServerMemberService,
    user: MockUserService,
}

impl Mocks {
    fn new() -> Self {
        Self {
            capability: MockCapabilityService::new(),
            conversation: MockConversationService::new(),
            message: MockMessageService::new(),
            message_reaction: MockMessageReactionService::new(),
            pin: MockPinService::new(),
            server_member: MockServerMemberService::new(),
            user: MockUserService::new(),
        }
    }
}

/// Build service with an explicit confirmation store (P6-M2: Agent+Mutating
/// commands require a pre-issued ConfirmationProof).
fn build_service_with_store(mocks: Mocks) -> (DefaultCommandService, Arc<dyn ConfirmationStore>) {
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
            capability_service: Arc::new(mocks.capability),
            conversation_service: Arc::new(mocks.conversation),
            message_service: Arc::new(mocks.message),
            message_reaction_service: Arc::new(mocks.message_reaction),
            pin_service: Arc::new(mocks.pin),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(mocks.server_member),
            user_service: Arc::new(mocks.user),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: std::sync::Arc::new(
                rim_services::incident::service::MockIncidentService::new(),
            ),
            oncall_service: std::sync::Arc::new(
                rim_services::oncall::service::MockOnCallService::new(),
            ),
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
        Arc::clone(&store),
    );
    (svc, store)
}

fn build_service(mocks: Mocks) -> DefaultCommandService {
    build_service_with_store(mocks).0
}

fn grant_all_caps() -> MockCapabilityService {
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

fn deny_all_caps() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check_many().returning(|_, _, caps| {
        let mut out = HashMap::with_capacity(caps.len());
        for c in caps {
            out.insert(c.clone(), false);
        }
        Ok(out)
    });
    cap
}

fn msg(
    id: MessageId,
    server: ServerId,
    conv: ConversationId,
    author: UserId,
    text: &str,
) -> MessageDto {
    MessageDto {
        id,
        server_id: server,
        conversation_id: conv,
        thread_id: None,
        root_thread_id: None,
        author_id: author,
        text: Some(text.to_string()),
        has_attachments: false,
        attachments: vec![],
        parent_message_id: None,
        forwarded_from_message_id: None,
        thread_root_message_id: None,
        is_deleted: false,
        sent_at: chrono::Utc::now(),
        encryption: None,
    }
}

/// Wraps a [`MessageDto`] into a search hit with a nominal rank and no headline — the exact
/// rank/headline values don't matter for these DataLoader-batching tests.
fn hit(message: MessageDto) -> MessageSearchHitDto {
    MessageSearchHitDto {
        message,
        rank: 1.0,
        headline: None,
    }
}

fn user_dto(id: UserId, username: &str) -> UserDto {
    UserDto {
        id,
        username: username.to_string(),
        display_name: Some(format!("display-{username}")),
        avatar_url: Some(format!("https://example/{username}.png")),
        status_text: None,
        bio: None,
        principal_id: None,
    }
}

// =====================================================================
// AI surface listing — all 5 commands gated on caps
// =====================================================================

#[tokio::test]
async fn ai_surface_lists_all_five_p4_a5_commands_when_caps_granted() {
    let mut mocks = Mocks::new();
    mocks.capability = grant_all_caps();
    let svc = build_service(mocks);

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .expect("AI surface listing must succeed when all caps granted");

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    for expected in [
        "chat.list_members",
        "chat.search_messages",
        "chat.get_recent_messages",
        "chat.pin_message",
        "chat.toggle_reaction",
    ] {
        assert!(
            ids.contains(&expected),
            "P4-A5 ai_callable command {expected} missing from Ai surface; got: {ids:?}"
        );
    }
}

#[tokio::test]
async fn ai_surface_omits_all_five_p4_a5_commands_when_caps_denied() {
    let mut mocks = Mocks::new();
    mocks.capability = deny_all_caps();
    let svc = build_service(mocks);

    let listed = svc
        .list_commands(ListCommandsQuery {
            actor: actor(),
            context: None,
            surface: CommandSurfaceDto::Ai,
        })
        .await
        .unwrap();

    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    for denied in [
        "chat.list_members",
        "chat.search_messages",
        "chat.get_recent_messages",
        "chat.pin_message",
        "chat.toggle_reaction",
    ] {
        assert!(
            !ids.contains(&denied),
            "{denied} MUST be filtered out when actor lacks required cap; got: {ids:?}"
        );
    }
}

// =====================================================================
// chat.list_members
// =====================================================================

#[tokio::test]
async fn execute_list_members_projects_username_role_joined_and_drops_pii() {
    let workspace = ServerId::new();
    let secret_user_id = UserId::new();

    let mut mocks = Mocks::new();
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, cap| cap == "chat.read")
        .returning(|_, _, _| Ok(()));
    mocks.capability = cap;

    let mut sm = MockServerMemberService::new();
    let ws = workspace;
    sm.expect_list_members()
        .withf(move |q| q.server_id == ws)
        .return_once(move |_q| {
            Box::pin(async move {
                Ok(vec![
                    ServerMemberDto {
                        server_id: ws,
                        user_id: secret_user_id,
                        username: "alice".to_string(),
                        display_name: Some("Alice Full-Name".to_string()),
                        avatar_url: Some("https://example/alice.png".to_string()),
                        status_text: Some("away".to_string()),
                        bio: Some("secret bio".to_string()),
                        member_role: rim_core::server_member::model::ServerMemberRole::Moderator,
                        joined_at: chrono::Utc::now(),
                        timeout_until: None,
                        principal_id: None,
                    },
                    ServerMemberDto {
                        server_id: ws,
                        user_id: UserId::new(),
                        username: "bob".to_string(),
                        display_name: None,
                        avatar_url: None,
                        status_text: None,
                        bio: None,
                        member_role: rim_core::server_member::model::ServerMemberRole::Member,
                        joined_at: chrono::Utc::now(),
                        timeout_until: None,
                        principal_id: None,
                    },
                ])
            })
        });
    mocks.server_member = sm;

    let svc = build_service(mocks);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.list_members".to_string(),
            args: Some(serde_json::json!({"server_id": workspace.to_string()})),
            context: None,
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.list_members via Agent must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("payload must be present");
    let members = payload
        .get("members")
        .and_then(|v| v.as_array())
        .expect("payload must carry a `members` array");
    assert_eq!(members.len(), 2);

    for m in members {
        // Allowed fields only.
        assert!(m.get("username").and_then(|v| v.as_str()).is_some());
        assert!(m.get("role").and_then(|v| v.as_str()).is_some());
        assert!(m.get("joined").and_then(|v| v.as_str()).is_some());

        // PII no-leak guard — Widow §7 no-go. Same pattern as
        // `chat.list_channels` → member_ids assertion.
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
                "{banned} MUST NOT appear in chat.list_members payload (PII leak); got: {m}"
            );
        }
    }
}

// =====================================================================
// chat.search_messages
// =====================================================================

#[tokio::test]
async fn execute_search_messages_batches_user_and_conversation_lookups_once() {
    // Core DataLoader assertion: N results with N distinct authors + N
    // distinct channels must trigger EXACTLY ONE `get_users_by_ids` and
    // ONE `get_conversations_by_ids`. mockall's `return_once` makes a
    // second call panic — if a future refactor drops the batch helper
    // and falls back to per-message lookups, this test catches it.
    let workspace = ServerId::new();
    let conv_a = ConversationId::new();
    let conv_b = ConversationId::new();
    let author_a = UserId::new();
    let author_b = UserId::new();
    let msg_a = MessageId::new();
    let msg_b = MessageId::new();

    let mut mocks = Mocks::new();
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, cap| cap == "chat.read")
        .returning(|_, _, _| Ok(()));
    mocks.capability = cap;

    let mut msg_svc = MockMessageService::new();
    msg_svc
        .expect_search_messages()
        .withf(move |q| q.query == "auth" && q.server_id == workspace && q.limit <= 50)
        .return_once(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![
                        hit(msg(
                            msg_a,
                            workspace,
                            conv_a,
                            author_a,
                            "auth migration done",
                        )),
                        hit(msg(
                            msg_b,
                            workspace,
                            conv_b,
                            author_b,
                            "auth follow-up tomorrow",
                        )),
                    ],
                    None,
                    None,
                ))
            })
        });
    mocks.message = msg_svc;

    let mut user_svc = MockUserService::new();
    user_svc
        .expect_get_users_by_ids()
        .times(1) // exactly one call — DataLoader assertion
        .withf(move |ids| ids.len() == 2 && ids.contains(&author_a) && ids.contains(&author_b))
        .return_once(move |_ids| {
            Box::pin(
                async move { Ok(vec![user_dto(author_a, "alice"), user_dto(author_b, "bob")]) },
            )
        });
    mocks.user = user_svc;

    let mut conv_svc = MockConversationService::new();
    conv_svc
        .expect_get_conversations_by_ids()
        .times(1) // exactly one call — DataLoader assertion
        .withf(move |ids| ids.len() == 2 && ids.contains(&conv_a) && ids.contains(&conv_b))
        .return_once(move |_ids| {
            Box::pin(async move {
                Ok(vec![
                    ConversationDto {
                        id: conv_a,
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
                    },
                    ConversationDto {
                        id: conv_b,
                        server_id: workspace,
                        conversation_type: ConversationType::Channel,
                        title: Some("engineering".to_string()),
                        topic: None,
                        member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                    },
                ])
            })
        });
    mocks.conversation = conv_svc;

    let svc = build_service(mocks);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.search_messages".to_string(),
            args: Some(serde_json::json!({
                "server_id": workspace.to_string(),
                "query": "auth",
            })),
            context: None,
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.search_messages via Agent must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("payload");
    let results = payload
        .get("results")
        .and_then(|v| v.as_array())
        .expect("results array");
    assert_eq!(results.len(), 2);

    // Wire-shape: {id, channel, author, text, sentAt}.
    for r in results {
        for field in ["id", "channel", "author", "text", "sentAt"] {
            assert!(
                r.get(field).is_some(),
                "search_messages result missing `{field}`: {r}"
            );
        }
    }
    // Author resolution actually happened (not just "", which would pass
    // structure check but hide a broken lookup).
    let authors: Vec<&str> = results
        .iter()
        .filter_map(|r| r.get("author").and_then(|v| v.as_str()))
        .collect();
    assert!(authors.contains(&"alice"));
    assert!(authors.contains(&"bob"));
    let channels: Vec<&str> = results
        .iter()
        .filter_map(|r| r.get("channel").and_then(|v| v.as_str()))
        .collect();
    assert!(channels.contains(&"general"));
    assert!(channels.contains(&"engineering"));
}

#[tokio::test]
async fn execute_search_messages_clamps_limit_over_max() {
    let workspace = ServerId::new();
    let mut mocks = Mocks::new();
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, cap| cap == "chat.read")
        .returning(|_, _, _| Ok(()));
    mocks.capability = cap;

    let mut msg_svc = MockMessageService::new();
    msg_svc
        .expect_search_messages()
        .withf(|q| q.limit == 50) // clamped from 999
        .return_once(|_q| Box::pin(async { Ok(Page::new(vec![], None, None)) }));
    mocks.message = msg_svc;
    // empty result — no user/conv lookups needed.

    let svc = build_service(mocks);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.search_messages".to_string(),
            args: Some(serde_json::json!({
                "server_id": workspace.to_string(),
                "query": "anything",
                "limit": 999,
            })),
            context: None,
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("clamped limit must still succeed");
    assert_eq!(result.status, CommandResultStatusDto::Success);
}

// =====================================================================
// chat.get_recent_messages
// =====================================================================

#[tokio::test]
async fn execute_get_recent_messages_batches_author_lookup_once() {
    let workspace = ServerId::new();
    let conv = ConversationId::new();
    let author_a = UserId::new();
    let author_b = UserId::new();

    let mut mocks = Mocks::new();
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, cap| cap == "chat.read")
        .returning(|_, _, _| Ok(()));
    mocks.capability = cap;

    let mut msg_svc = MockMessageService::new();
    msg_svc
        .expect_list_messages()
        .withf(move |q| {
            q.server_id == workspace
                && q.conversation_id == conv
                && !q.include_deleted
                && q.limit <= 100
        })
        .return_once(move |_q| {
            Box::pin(async move {
                Ok(rim_core::repository::pagination::Page::new(
                    vec![
                        msg(MessageId::new(), workspace, conv, author_a, "hi team"),
                        msg(MessageId::new(), workspace, conv, author_b, "hey back"),
                        msg(MessageId::new(), workspace, conv, author_a, "ship it"),
                    ],
                    None,
                    None,
                ))
            })
        });
    mocks.message = msg_svc;

    let mut user_svc = MockUserService::new();
    // DataLoader assertion: exactly one call with the 2 distinct authors
    // (not 3 — the executor dedups).
    user_svc
        .expect_get_users_by_ids()
        .times(1)
        .withf(move |ids| ids.len() == 2)
        .return_once(move |_ids| {
            Box::pin(
                async move { Ok(vec![user_dto(author_a, "alice"), user_dto(author_b, "bob")]) },
            )
        });
    mocks.user = user_svc;

    let svc = build_service(mocks);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.get_recent_messages".to_string(),
            args: Some(serde_json::json!({
                "server_id": workspace.to_string(),
                "conversation_id": conv.to_string(),
            })),
            context: None,
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.get_recent_messages via Agent must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("payload");
    let msgs = payload
        .get("messages")
        .and_then(|v| v.as_array())
        .expect("messages array");
    assert_eq!(msgs.len(), 3);
    for m in msgs {
        for field in ["author", "text", "sentAt"] {
            assert!(
                m.get(field).is_some(),
                "missing `{field}` in recent message: {m}"
            );
        }
    }
}

// =====================================================================
// chat.pin_message
// =====================================================================

#[tokio::test]
async fn execute_pin_message_calls_pin_service_and_returns_success_payload() {
    let workspace = ServerId::new();
    let conv = ConversationId::new();
    let msg_id = MessageId::new();

    let mut mocks = Mocks::new();
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_, _, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));
    mocks.capability = cap;

    let mut pin = MockPinService::new();
    pin.expect_pin_message()
        .withf(move |cmd| {
            cmd.server_id == workspace && cmd.conversation_id == conv && cmd.message_id == msg_id
        })
        .return_once(|_cmd| Box::pin(async { Ok(()) }));
    mocks.pin = pin;

    use rim_services::command::confirmation::ConfirmationProof;
    let actor = actor();
    let args = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "conversation_id": conv.to_string(),
        "message_id": msg_id.to_string(),
    }));

    let (svc, store) = build_service_with_store(mocks);

    // P6-M2: chat.pin_message is Mutating → Agent must supply a proof.
    let challenge = issue_in_band(store.as_ref(), &actor, "chat.pin_message", &args, &None);
    let proof = ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor,
            command_id: "chat.pin_message".to_string(),
            args,
            context: None,
            confirmation: Some(proof),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("chat.pin_message via Agent must succeed");

    assert_eq!(result.status, CommandResultStatusDto::Success);
    let payload = result.payload.expect("payload");
    assert_eq!(payload["success"], true);
    assert_eq!(payload["pinned"], msg_id.to_string());
}

// =====================================================================
// chat.toggle_reaction — bidirectional semantics
// =====================================================================

#[tokio::test]
async fn execute_toggle_reaction_flips_action_added_then_removed() {
    // Tony's P4-A5 naming decision: the tool's payload `action` must flip
    // from "added" to "removed" on two successive calls with the same
    // (message, emoji, actor). Locks the bidirectional contract; any
    // refactor that hardcodes `action: "added"` regresses the LLM's
    // ability to reason about the effect of its own call.
    let workspace = ServerId::new();
    let conv = ConversationId::new();
    let msg_id = MessageId::new();
    let actor_uid = UserId::new();

    // --- First call: no prior reaction → action: "added" -----------------
    let mut mocks1 = Mocks::new();
    let mut cap1 = MockCapabilityService::new();
    cap1.expect_check()
        .withf(|_, _, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));
    mocks1.capability = cap1;

    let mut reaction1 = MockMessageReactionService::new();
    // get_reactions returns empty (no reactions yet)
    reaction1
        .expect_get_reactions()
        .return_once(|_q| Box::pin(async { Ok(vec![]) }));
    reaction1
        .expect_toggle_reaction()
        .withf(move |cmd| cmd.message_id == msg_id && cmd.emoji == "👍")
        .return_once(move |_cmd| {
            Box::pin(async move {
                Ok(vec![ReactionGroupDto {
                    emoji: "👍".to_string(),
                    count: 1,
                    reacted_by_me: true,
                    user_ids: vec![actor_uid],
                }])
            })
        });
    mocks1.message_reaction = reaction1;

    use rim_services::command::confirmation::ConfirmationProof;
    let actor1 = actor_with_id(actor_uid);
    let args1 = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "conversation_id": conv.to_string(),
        "message_id": msg_id.to_string(),
        "emoji": "👍",
    }));

    let (svc1, store1) = build_service_with_store(mocks1);
    // P6-M2: chat.toggle_reaction is Mutating → Agent must supply a proof.
    let challenge1 = issue_in_band(
        store1.as_ref(),
        &actor1,
        "chat.toggle_reaction",
        &args1,
        &None,
    );
    let proof1 = ConfirmationProof::InBandToken {
        token: challenge1.token,
        args_hash: challenge1.args_hash,
    };
    let result1 = svc1
        .execute_command(ExecuteCommandCmd {
            actor: actor1,
            command_id: "chat.toggle_reaction".to_string(),
            args: args1,
            context: None,
            confirmation: Some(proof1),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("first toggle must succeed");

    assert_eq!(result1.status, CommandResultStatusDto::Success);
    let payload1 = result1.payload.expect("payload");
    assert_eq!(payload1["success"], true);
    assert_eq!(payload1["action"], "added");
    assert_eq!(payload1["emoji"], "👍");
    assert_eq!(payload1["toggled"], msg_id.to_string());

    // --- Second call: actor already reacted → action: "removed" ----------
    let mut mocks2 = Mocks::new();
    let mut cap2 = MockCapabilityService::new();
    cap2.expect_check()
        .withf(|_, _, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));
    mocks2.capability = cap2;

    let mut reaction2 = MockMessageReactionService::new();
    // get_reactions reports actor already reacted with 👍
    reaction2.expect_get_reactions().return_once(move |_q| {
        Box::pin(async move {
            Ok(vec![ReactionGroupDto {
                emoji: "👍".to_string(),
                count: 1,
                reacted_by_me: true,
                user_ids: vec![actor_uid],
            }])
        })
    });
    reaction2
        .expect_toggle_reaction()
        .return_once(|_cmd| Box::pin(async { Ok(vec![]) }));
    mocks2.message_reaction = reaction2;

    let actor2 = actor_with_id(actor_uid);
    let args2 = Some(serde_json::json!({
        "server_id": workspace.to_string(),
        "conversation_id": conv.to_string(),
        "message_id": msg_id.to_string(),
        "emoji": "👍",
    }));

    let (svc2, store2) = build_service_with_store(mocks2);
    let challenge2 = issue_in_band(
        store2.as_ref(),
        &actor2,
        "chat.toggle_reaction",
        &args2,
        &None,
    );
    let proof2 = ConfirmationProof::InBandToken {
        token: challenge2.token,
        args_hash: challenge2.args_hash,
    };
    let result2 = svc2
        .execute_command(ExecuteCommandCmd {
            actor: actor2,
            command_id: "chat.toggle_reaction".to_string(),
            args: args2,
            context: None,
            confirmation: Some(proof2),
            is_pipe: false,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("second toggle must succeed");

    assert_eq!(result2.status, CommandResultStatusDto::Success);
    let payload2 = result2.payload.expect("payload");
    assert_eq!(payload2["action"], "removed");
    assert_eq!(payload2["emoji"], "👍");
    assert_eq!(payload2["toggled"], msg_id.to_string());
}
