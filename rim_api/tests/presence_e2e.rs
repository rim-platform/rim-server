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
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::WorkspaceEvent;

use support::graphql::{Caller, anonymous_request, assert_each_refused, exec_err_code, exec_ok};
use support::roles::{Roles, create_channel};
use support::{RecordingBroadcaster, TestCtx};

const SET_ARCHIVED: &str = r#"
mutation Archive($serverId: ID!, $conversationId: ID!) {
  conversation: setConversationArchived(serverId: $serverId, conversationId: $conversationId,
                                        archived: true) { id }
}
"#;

const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  addConversationMembers(serverId: $serverId, conversationId: $conversationId,
                         userIds: $userIds) { id }
}
"#;

const TIMEOUT: &str = r#"
mutation Timeout($serverId: ID!, $userId: ID!, $until: DateTime!) {
  timeoutMember(serverId: $serverId, userId: $userId, until: $until) { userId }
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
    ws: Roles,
    /// Public channel alice created.
    channel_id: String,
    _ctx: TestCtx,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("presence").await;
    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    let channel_id = create_channel(&schema, &ws.alice, &ws.server_id, "general", false).await;
    Fixture {
        schema,
        events,
        ws,
        channel_id,
        _ctx: ctx,
    }
}

impl Fixture {
    fn heartbeat_vars(&self, status: &str) -> Value {
        json!({ "workspaceId": self.ws.server_id, "status": status })
    }

    async fn heartbeat(&self, who: &Caller, status: &str) {
        let data = exec_ok(
            &self.schema,
            who.request(HEARTBEAT, self.heartbeat_vars(status)),
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
        json!({ "serverId": self.ws.server_id, "conversationId": conversation_id })
    }

    async fn typing_code(&self, who: &Caller, conversation_id: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(START_TYPING, self.typing_vars(conversation_id)),
        )
        .await
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
        (HEARTBEAT, f.heartbeat_vars("ONLINE")),
        (START_TYPING, f.typing_vars(&f.channel_id)),
        (WORKSPACE_PRESENCE, json!({ "workspaceId": f.ws.server_id })),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// A user outside the workspace may not publish presence or typing into it.
#[tokio::test]
async fn presence_writes_refuse_non_members() {
    let f = setup().await;
    let calls = [
        (HEARTBEAT, f.heartbeat_vars("ONLINE")),
        (START_TYPING, f.typing_vars(&f.channel_id)),
    ];
    assert_each_refused(&f.schema, &calls, "FORBIDDEN", |q, v| {
        f.ws.olive.request(q, v)
    })
    .await;
    assert!(f.presence_changes().is_empty());
    assert!(f.typing_events().is_empty());
}

/// A heartbeat makes the member visible to other members of that workspace with the status they
/// sent, a guest included, and emits one presence change per member.
#[tokio::test]
async fn heartbeat_shows_the_status_to_other_members() {
    let f = setup().await;
    assert!(f.presence(&f.ws.alice, &f.ws.server_id).await.is_empty());

    let before = chrono::Utc::now();
    f.heartbeat(&f.ws.bob, "ONLINE").await;
    f.heartbeat(&f.ws.gina, "AWAY").await;
    let after = chrono::Utc::now();

    let mut seen = f.presence(&f.ws.alice, &f.ws.server_id).await;
    seen.sort_by_key(|p| p["status"].as_str().unwrap_or_default().to_string());
    assert_eq!(seen.len(), 2, "{seen:?}");
    let expected = [(&f.ws.gina, "AWAY"), (&f.ws.bob, "ONLINE")];
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
            (f.ws.bob.user_id.to_string(), "Online".to_string()),
            (f.ws.gina.user_id.to_string(), "Away".to_string()),
        ]
    );
}

/// A guest sees the presence of members they share a conversation with and of nobody else;
/// owners and members see everyone.
#[tokio::test]
async fn guest_sees_presence_only_of_members_sharing_a_conversation() {
    let f = setup().await;
    let shared = create_channel(&f.schema, &f.ws.alice, &f.ws.server_id, "guests", true).await;
    exec_ok(
        &f.schema,
        f.ws.alice.request(
            ADD_MEMBERS,
            json!({
                "serverId": f.ws.server_id,
                "conversationId": shared,
                "userIds": [f.ws.gina.user_id.to_string()],
            }),
        ),
    )
    .await;
    for who in [&f.ws.alice, &f.ws.bob, &f.ws.gina] {
        f.heartbeat(who, "ONLINE").await;
    }

    let users = |entries: Vec<Value>| {
        let mut ids: Vec<String> = entries
            .iter()
            .map(|e| e["userId"].as_str().unwrap_or_default().to_string())
            .collect();
        ids.sort();
        ids
    };
    let ids = |callers: &[&Caller]| {
        let mut ids: Vec<String> = callers.iter().map(|c| c.user_id.to_string()).collect();
        ids.sort();
        ids
    };
    assert_eq!(
        users(f.presence(&f.ws.gina, &f.ws.server_id).await),
        ids(&[&f.ws.alice, &f.ws.gina])
    );
    let everyone = ids(&[&f.ws.alice, &f.ws.bob, &f.ws.gina]);
    for who in [&f.ws.alice, &f.ws.bob] {
        assert_eq!(users(f.presence(who, &f.ws.server_id).await), everyone);
    }
}

/// Presence is per workspace: a heartbeat into one workspace does not show the member in
/// another workspace's presence list.
#[tokio::test]
async fn presence_is_scoped_to_the_workspace_heartbeated_into() {
    let f = setup().await;
    f.heartbeat(&f.ws.bob, "ONLINE").await;
    assert!(
        f.presence(&f.ws.olive, &f.ws.other_server_id)
            .await
            .is_empty()
    );
}

/// Clients cannot report themselves `OFFLINE` (`VALIDATION`); the refusal changes nothing.
#[tokio::test]
async fn heartbeat_refuses_offline_status() {
    let f = setup().await;
    let code = exec_err_code(
        &f.schema,
        f.ws.bob.request(HEARTBEAT, f.heartbeat_vars("OFFLINE")),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(f.presence(&f.ws.alice, &f.ws.server_id).await.is_empty());
    assert!(f.presence_changes().is_empty());
}

/// A second heartbeat inside the rate-limit window is accepted but ignored: the first status
/// stays and no second change is emitted.
#[tokio::test]
async fn heartbeat_inside_the_rate_limit_window_keeps_the_first_status() {
    let f = setup().await;
    f.heartbeat(&f.ws.bob, "ONLINE").await;
    f.heartbeat(&f.ws.bob, "AWAY").await;

    let seen = f.presence(&f.ws.alice, &f.ws.server_id).await;
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
            f.ws.bob.request(START_TYPING, f.typing_vars(&f.channel_id)),
        )
        .await;
        assert_eq!(data["startTyping"], json!(true));
    }
    assert_eq!(
        f.typing_events(),
        vec![(
            f.ws.server_id.clone(),
            f.channel_id.clone(),
            f.ws.bob.user_id.to_string()
        )]
    );
}

/// `startTyping` answers `NOT_FOUND` for a conversation that does not exist, one in another
/// workspace and a private channel the caller is not in, and emits nothing for them.
#[tokio::test]
async fn start_typing_refuses_conversations_the_caller_cannot_see() {
    let f = setup().await;
    let missing = uuid::Uuid::now_v7().to_string();
    let foreign = create_channel(&f.schema, &f.ws.olive, &f.ws.other_server_id, "x", false).await;
    let private = create_channel(&f.schema, &f.ws.alice, &f.ws.server_id, "staff", true).await;

    for conversation_id in [&missing, &foreign, &private] {
        assert_eq!(f.typing_code(&f.ws.bob, conversation_id).await, "NOT_FOUND");
    }
    assert!(f.typing_events().is_empty());
}

/// Typing needs write access: a timed-out member gets `TIMED_OUT` and an archived channel
/// `CONVERSATION_ARCHIVED`, and neither emits an event.
#[tokio::test]
async fn start_typing_refuses_where_the_caller_may_not_write() {
    let f = setup().await;
    let until = chrono::Utc::now() + chrono::Duration::minutes(10);
    exec_ok(
        &f.schema,
        f.ws.mona.request(
            TIMEOUT,
            json!({
                "serverId": f.ws.server_id,
                "userId": f.ws.carol.user_id.to_string(),
                "until": until.to_rfc3339(),
            }),
        ),
    )
    .await;
    assert_eq!(f.typing_code(&f.ws.carol, &f.channel_id).await, "TIMED_OUT");

    exec_ok(
        &f.schema,
        f.ws.alice
            .request(SET_ARCHIVED, f.typing_vars(&f.channel_id)),
    )
    .await;
    assert_eq!(
        f.typing_code(&f.ws.bob, &f.channel_id).await,
        "CONVERSATION_ARCHIVED"
    );
    assert!(f.typing_events().is_empty());
}
