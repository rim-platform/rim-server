//! Through-the-stack e2e tests for the presence and typing GraphQL contract: `heartbeat`,
//! `startTyping` and `workspacePresence`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::WorkspaceEvent;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
use support::{RecordingBroadcaster, TestCtx};

const CREATE_CHANNEL: &str = r#"
mutation Create($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

const SET_ARCHIVED: &str = r#"
mutation Archive($serverId: ID!, $conversationId: ID!, $archived: Boolean!) {
  conversation: setConversationArchived(serverId: $serverId, conversationId: $conversationId,
                                        archived: $archived) { id }
}
"#;

const HEARTBEAT: &str = r#"
mutation Heartbeat($workspaceId: ID!, $status: PresenceStatus!) {
  heartbeat(input: { workspaceId: $workspaceId, status: $status })
}
"#;

const START_TYPING: &str = r#"
mutation Typing($serverId: ID!, $conversationId: ID!) {
  startTyping(serverId: $serverId, conversationId: $conversationId)
}
"#;

const WORKSPACE_PRESENCE: &str = r#"
query Presence($workspaceId: ID!) {
  workspacePresence(workspaceId: $workspaceId) { userId status lastSeenAt }
}
"#;

struct Fixture {
    schema: AppSchema,
    events: Arc<RecordingBroadcaster>,
    server_id: String,
    other_server_id: String,
    /// Owner; created the public channel.
    alice: Caller,
    bob: Caller,
    gina: Caller,
    /// Member of the other workspace only.
    olaf: Caller,
    channel_id: String,
    _ctx: TestCtx,
}

async fn caller(ctx: &TestCtx, user: &User) -> Caller {
    Caller {
        user_id: user.id,
        session_id: ctx.create_session(user).await,
    }
}

/// alice (Owner), bob (Member) and gina (Guest) in one workspace with a public channel alice
/// created; olaf owns a second workspace.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("presence-e2e").await;
    let other = ctx.create_server("presence-e2e-other").await;
    let alice_user = ctx.create_user("presence-alice").await;
    let bob_user = ctx.create_user("presence-bob").await;
    let gina_user = ctx.create_user("presence-gina").await;
    let olaf_user = ctx.create_user("presence-olaf").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina_user, ServerMemberRole::Guest)
        .await;
    ctx.add_owner(&other, &olaf_user).await;

    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    let alice = caller(&ctx, &alice_user).await;
    let server_id = server.id.to_string();
    let created = exec_ok(
        &schema,
        alice.request(
            CREATE_CHANNEL,
            json!({ "input": { "serverId": server_id, "isChannel": true, "title": "general" } }),
        ),
    )
    .await;
    let channel_id = created["conversation"]["id"]
        .as_str()
        .expect("channel id")
        .to_string();

    Fixture {
        bob: caller(&ctx, &bob_user).await,
        gina: caller(&ctx, &gina_user).await,
        olaf: caller(&ctx, &olaf_user).await,
        alice,
        schema,
        events,
        server_id,
        other_server_id: other.id.to_string(),
        channel_id,
        _ctx: ctx,
    }
}

impl Fixture {
    async fn heartbeat(&self, who: &Caller, status: &str) {
        let data = exec_ok(
            &self.schema,
            who.request(
                HEARTBEAT,
                json!({ "workspaceId": self.server_id, "status": status }),
            ),
        )
        .await;
        assert_eq!(data["heartbeat"], json!(true));
    }

    async fn presence(&self, who: &Caller, workspace_id: &str) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(WORKSPACE_PRESENCE, json!({ "workspaceId": workspace_id })),
        )
        .await;
        data["workspacePresence"]
            .as_array()
            .expect("presence list")
            .clone()
    }

    fn typing_vars(&self, conversation_id: &str) -> Value {
        json!({ "serverId": self.server_id, "conversationId": conversation_id })
    }

    /// `(user, status)` of every presence change emitted so far.
    fn presence_changes(&self) -> Vec<(String, String)> {
        self.events
            .events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::PresenceChanged {
                    user_id, status, ..
                } => Some((user_id.to_string(), status)),
                _ => None,
            })
            .collect()
    }

    /// `(workspace, conversation, user)` of every typing event emitted so far.
    fn typing_events(&self) -> Vec<(String, String, String)> {
        self.events
            .events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::TypingStarted {
                    workspace_id,
                    conversation_id,
                    user_id,
                    ..
                } => Some((
                    workspace_id.to_string(),
                    conversation_id.to_string(),
                    user_id.to_string(),
                )),
                _ => None,
            })
            .collect()
    }
}

/// Every presence root answers `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn presence_roots_refuse_anonymous_callers() {
    let f = setup().await;
    let calls = [
        (
            HEARTBEAT,
            json!({ "workspaceId": f.server_id, "status": "ONLINE" }),
        ),
        (START_TYPING, f.typing_vars(&f.channel_id)),
        (WORKSPACE_PRESENCE, json!({ "workspaceId": f.server_id })),
    ];
    for (query, vars) in calls {
        let code = exec_err_code(&f.schema, anonymous_request(query, vars)).await;
        assert_eq!(code, "UNAUTHENTICATED", "{query}");
    }
}

/// A user outside the workspace may neither publish presence or typing into it nor read its
/// presence.
#[tokio::test]
async fn presence_roots_refuse_non_members() {
    let f = setup().await;
    let calls = [
        (
            HEARTBEAT,
            json!({ "workspaceId": f.server_id, "status": "ONLINE" }),
        ),
        (START_TYPING, f.typing_vars(&f.channel_id)),
        (WORKSPACE_PRESENCE, json!({ "workspaceId": f.server_id })),
    ];
    for (query, vars) in calls {
        let code = exec_err_code(&f.schema, f.olaf.request(query, vars)).await;
        assert_eq!(code, "FORBIDDEN", "{query}");
    }
    assert!(f.presence_changes().is_empty());
    assert!(f.typing_events().is_empty());
}

/// A heartbeat makes the member visible to other members of that workspace with the status they
/// sent, a guest included, and emits one presence change per member.
#[tokio::test]
async fn heartbeat_shows_the_status_to_other_members() {
    let f = setup().await;
    assert!(f.presence(&f.alice, &f.server_id).await.is_empty());

    let before = chrono::Utc::now();
    f.heartbeat(&f.bob, "ONLINE").await;
    f.heartbeat(&f.gina, "AWAY").await;
    let after = chrono::Utc::now();

    let mut seen = f.presence(&f.alice, &f.server_id).await;
    seen.sort_by_key(|p| p["status"].as_str().unwrap_or_default().to_string());
    assert_eq!(seen.len(), 2, "{seen:?}");
    let expected = [(&f.gina, "AWAY"), (&f.bob, "ONLINE")];
    for (entry, (who, status)) in seen.iter().zip(expected) {
        assert_eq!(entry["userId"], json!(who.user_id.to_string()));
        assert_eq!(entry["status"], json!(status));
        let last_seen: chrono::DateTime<chrono::Utc> = entry["lastSeenAt"]
            .as_str()
            .expect("lastSeenAt")
            .parse()
            .expect("RFC 3339");
        assert!(last_seen >= before && last_seen <= after, "{last_seen}");
    }

    assert_eq!(
        f.presence_changes(),
        vec![
            (f.bob.user_id.to_string(), "Online".to_string()),
            (f.gina.user_id.to_string(), "Away".to_string()),
        ]
    );
}

/// Presence is per workspace: a heartbeat into one workspace does not show the member in
/// another workspace's presence list.
#[tokio::test]
async fn presence_is_scoped_to_the_workspace_heartbeated_into() {
    let f = setup().await;
    f.heartbeat(&f.bob, "ONLINE").await;
    assert!(f.presence(&f.olaf, &f.other_server_id).await.is_empty());
}

/// Clients cannot report themselves `OFFLINE`; the refusal changes nothing.
#[tokio::test]
async fn heartbeat_refuses_offline_status() {
    let f = setup().await;
    let res = f
        .schema
        .execute(f.bob.request(
            HEARTBEAT,
            json!({ "workspaceId": f.server_id, "status": "OFFLINE" }),
        ))
        .await;
    let err = res.errors.first().expect("an error");
    assert_eq!(err.message, "Clients cannot set OFFLINE status");
    assert!(f.presence(&f.alice, &f.server_id).await.is_empty());
    assert!(f.presence_changes().is_empty());
}

/// A second heartbeat inside the rate-limit window is accepted but ignored: the first status
/// stays and no second change is emitted.
#[tokio::test]
async fn heartbeat_inside_the_rate_limit_window_keeps_the_first_status() {
    let f = setup().await;
    f.heartbeat(&f.bob, "ONLINE").await;
    f.heartbeat(&f.bob, "AWAY").await;

    let seen = f.presence(&f.alice, &f.server_id).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["status"], json!("ONLINE"));
    assert_eq!(f.presence_changes().len(), 1);
}

/// `startTyping` answers `true` and emits one typing event naming the workspace, conversation
/// and typist; a repeat inside the throttle window answers `true` and emits nothing.
#[tokio::test]
async fn start_typing_emits_one_event_per_throttle_window() {
    let f = setup().await;
    for _ in 0..2 {
        let data = exec_ok(
            &f.schema,
            f.bob.request(START_TYPING, f.typing_vars(&f.channel_id)),
        )
        .await;
        assert_eq!(data["startTyping"], json!(true));
    }
    assert_eq!(
        f.typing_events(),
        vec![(
            f.server_id.clone(),
            f.channel_id.clone(),
            f.bob.user_id.to_string()
        )]
    );
}

/// `startTyping` answers `NOT_FOUND` for a conversation that does not exist, one in another
/// workspace and a private channel the caller is not in, and emits nothing for them.
#[tokio::test]
async fn start_typing_refuses_conversations_the_caller_cannot_see() {
    let f = setup().await;

    let missing = uuid::Uuid::now_v7().to_string();
    let code = exec_err_code(
        &f.schema,
        f.bob.request(START_TYPING, f.typing_vars(&missing)),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");

    let foreign = exec_ok(
        &f.schema,
        f.olaf.request(
            CREATE_CHANNEL,
            json!({ "input": { "serverId": f.other_server_id, "isChannel": true, "title": "x" } }),
        ),
    )
    .await;
    let foreign_id = foreign["conversation"]["id"].as_str().expect("id");
    let code = exec_err_code(
        &f.schema,
        f.bob.request(START_TYPING, f.typing_vars(foreign_id)),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");

    let private = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CHANNEL,
            json!({ "input": {
                "serverId": f.server_id, "isChannel": true, "isPrivate": true, "title": "staff",
            } }),
        ),
    )
    .await;
    let private_id = private["conversation"]["id"].as_str().expect("id");
    let code = exec_err_code(
        &f.schema,
        f.bob.request(START_TYPING, f.typing_vars(private_id)),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");

    assert!(f.typing_events().is_empty());
}

/// Typing into an archived channel answers `CONVERSATION_ARCHIVED`.
#[tokio::test]
async fn start_typing_refuses_an_archived_channel() {
    let f = setup().await;
    exec_ok(
        &f.schema,
        f.alice.request(
            SET_ARCHIVED,
            json!({ "serverId": f.server_id, "conversationId": f.channel_id, "archived": true }),
        ),
    )
    .await;
    let code = exec_err_code(
        &f.schema,
        f.bob.request(START_TYPING, f.typing_vars(&f.channel_id)),
    )
    .await;
    assert_eq!(code, "CONVERSATION_ARCHIVED");
    assert!(f.typing_events().is_empty());
}
