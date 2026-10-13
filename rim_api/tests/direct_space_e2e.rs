//! The roots a client sends against the direct space, each called by an account that has never
//! joined anything and never read `relayInfo` with its session. A workspace-scoped root is a
//! first use: it makes the caller a guest of the direct space and answers as it does for any
//! guest, an empty list where they have nothing yet. Roots that need no membership join nobody.
//! Workspace-only features answer `VALIDATION` there and join nobody either.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use async_graphql::Request;
use futures_util::StreamExt;
use rim_api::auth_context::AuthActorCtx;
use rim_services::NOT_IN_DIRECT_SPACE;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use support::nodes::{Node, databases_with, start};
use support::relay::{
    CREATE, Person, RESET, anonymous_direct_space, open_registration, sign_up, user_id,
};

/// A relay, a freshly signed-up account on it that has joined nothing, and the relay's direct
/// space.
struct Newcomer {
    node: Node,
    person: Person,
    direct: String,
}

impl Newcomer {
    async fn new() -> Self {
        let (database, _) = databases_with(RESET).await;
        let node = start(&database, open_registration).await;
        let person = sign_up(&node, "newcomer", 1).await;
        let direct = anonymous_direct_space(&node).await;
        let newcomer = Self {
            node,
            person,
            direct,
        };
        assert_eq!(newcomer.role_of(&newcomer.person).await, None);
        newcomer
    }

    /// `person`'s role in the direct space, `None` when they are not a member.
    async fn role_of(&self, person: &Person) -> Option<String> {
        let user: Uuid = user_id(&self.node, &person.principal()).await.into();
        let direct: Uuid = self.direct.parse().expect("uuid");
        self.node
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT member_role::text AS role FROM server_member \
                 WHERE server_id = $1 AND user_id = $2",
                [direct.into(), user.into()],
            ))
            .await
            .expect("query")
            .map(|row| row.try_get::<String>("", "role").expect("role"))
    }

    /// Run `query` with `$s` bound to the direct space; panics on errors.
    async fn ok(&self, query: &str) -> Value {
        self.person.ok(query, json!({ "s": self.direct })).await
    }

    /// Run `query` with `$s` bound to the direct space; asserts it is refused as a workspace-only
    /// feature.
    async fn refused(&self, query: &str) {
        let body = self.person.post(query, json!({ "s": self.direct })).await;
        let error = &body["errors"][0];
        assert_eq!(error["extensions"]["code"], "VALIDATION", "{body}");
        assert_eq!(error["message"], NOT_IN_DIRECT_SPACE, "{body}");
    }

    /// Assert the newcomer joined nothing, then stop the relay.
    async fn joined_nothing(self) {
        assert_eq!(self.role_of(&self.person).await, None);
        self.node.stop().await;
    }

    /// Assert the newcomer became a guest of the direct space, then stop the relay.
    async fn joined_as_guest(self) {
        assert_eq!(self.role_of(&self.person).await.as_deref(), Some("guest"));
        self.node.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unread_counts_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { unreadCounts(workspaceId: $s) { conversationId count } }")
        .await;
    assert_eq!(data["unreadCounts"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn my_notifications_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { myNotifications(serverId: $s) { items { id } next { notificationId } } }")
        .await;
    assert_eq!(
        data["myNotifications"],
        json!({ "items": [], "next": null })
    );
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn my_unread_notification_count_is_zero_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { myUnreadNotificationCount(serverId: $s) }")
        .await;
    assert_eq!(data["myUnreadNotificationCount"], json!(0));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn my_notification_settings_are_the_defaults_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { myNotificationSettings(serverId: $s) { keywords } }")
        .await;
    assert_eq!(data["myNotificationSettings"]["keywords"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn muted_conversations_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { mutedConversations(serverId: $s) { conversationId } }")
        .await;
    assert_eq!(data["mutedConversations"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_presence_is_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { workspacePresence(workspaceId: $s) { userId } }")
        .await;
    assert_eq!(data["workspacePresence"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_heartbeat_is_accepted_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("mutation($s: ID!) { heartbeat(input: { workspaceId: $s, status: ONLINE }) }")
        .await;
    assert_eq!(data["heartbeat"], json!(true));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_shows_a_guest_viewer_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { server(serverId: $s) { id viewerRole } }")
        .await;
    assert_eq!(
        data["server"],
        json!({ "id": n.direct, "viewerRole": "GUEST" })
    );
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_members_list_only_the_caller_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { serverMembers(serverId: $s) { principalId memberRole } }")
        .await;
    assert_eq!(
        data["serverMembers"],
        json!([{ "principalId": n.person.principal(), "memberRole": "GUEST" }])
    );
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channels_are_refused_in_the_direct_space() {
    let n = Newcomer::new().await;
    n.refused("query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 100) { items { id } } }")
        .await;
    n.refused(
        "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 100, \
         filter: { joinedOnly: true }) { items { id } } }",
    )
    .await;
    n.refused(
        "mutation($s: ID!) { createConversation(input: { serverId: $s, isChannel: true, \
         title: \"general\" }) { id } }",
    )
    .await;
    n.refused(&format!(
        "mutation($s: ID!) {{ joinChannel(serverId: $s, conversationId: \"{}\") {{ id }} }}",
        Uuid::now_v7()
    ))
    .await;
    n.joined_nothing().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_keybinds_are_refused_in_the_direct_space() {
    let n = Newcomer::new().await;
    n.refused("query($s: ID!) { resolvedKeybinds(workspaceId: $s) { workspaceId } }")
        .await;
    n.refused(
        "mutation($s: ID!) { createKeybind(input: { scope: WORKSPACE, workspaceId: $s, \
         keystroke: \"Ctrl+J\", commandId: \"workspace.open_command_palette\", \
         context: GLOBAL }) { id } }",
    )
    .await;
    n.joined_nothing().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_list_what_a_guest_runs_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { commands(commandContext: { workspaceId: $s }) { id } }")
        .await;
    let ids: Vec<&str> = data["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|c| c["id"].as_str().expect("id"))
        .collect();
    for id in [
        "chat.post_message",
        "chat.create_group",
        "chat.invite_to_conversation",
        "workspace.open_settings",
    ] {
        assert!(ids.contains(&id), "{id} missing: {ids:?}");
    }
    for id in ["chat.create_channel", "chat.archive_channel", "admin.kick"] {
        assert!(!ids.contains(&id), "{id} listed: {ids:?}");
    }
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_emojis_are_refused_in_the_direct_space() {
    let n = Newcomer::new().await;
    n.refused("query($s: ID!) { customEmojis(serverId: $s) { id } }")
        .await;
    n.refused(
        "mutation($s: ID!) { createCustomEmoji(input: { serverId: $s, name: \"wave\", \
         imageBase64: \"iVBORw0KGgo=\" }) { id } }",
    )
    .await;
    let emoji = Uuid::now_v7();
    let user: Uuid = user_id(&n.node, &n.person.principal()).await.into();
    let direct: Uuid = n.direct.parse().expect("uuid");
    n.node
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO custom_emoji (id, server_id, name, image_url, uploaded_by, created_at) \
             VALUES ($1, $2, 'wave', 'emoji/wave.png', $3, now())",
            [emoji.into(), direct.into(), user.into()],
        ))
        .await
        .expect("insert");
    n.refused(&format!(
        "mutation {{ deleteCustomEmoji(emojiId: \"{emoji}\") }}"
    ))
    .await;
    n.joined_nothing().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugins_pipelines_and_workspace_state_are_refused_in_the_direct_space() {
    let n = Newcomer::new().await;
    n.refused("query($s: ID!) { workspacePlugins(serverId: $s) { id } }")
        .await;
    n.refused(
        "mutation($s: ID!) { addWorkspacePlugin(input: { serverId: $s, \
         statement: { payload: \"e30\", signature: \"AA\" }, status: RECOMMENDED }) { id } }",
    )
    .await;
    n.refused(
        "mutation($s: ID!) { updateWorkspacePlugin(input: { serverId: $s, \
         pluginId: \"example\", status: REQUIRED }) { id } }",
    )
    .await;
    n.refused("mutation($s: ID!) { removeWorkspacePlugin(serverId: $s, pluginId: \"example\") }")
        .await;
    let upload = n
        .person
        .http
        .post(format!("{}/api/plugin-bundles/{}", n.node.origin, n.direct))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", n.person.session, n.person.csrf),
        )
        .header("x-csrf-token", &n.person.csrf)
        .header("content-type", "application/vnd.rim.plugin-bundle+json")
        .body(r#"{"format":"rim-plugin-bundle","v":1,"files":[{"path":"rim-plugin.json","text":"{}"}]}"#)
        .send()
        .await
        .expect("upload");
    assert_eq!(upload.status(), 400);
    let body: Value = upload.json().await.expect("json");
    assert_eq!(body["error"], NOT_IN_DIRECT_SPACE);

    n.refused("query($s: ID!) { pipelines(serverId: $s) { id } }")
        .await;
    n.refused(
        "mutation($s: ID!) { createPipeline(input: { serverId: $s, name: \"echo\", \
         trigger: { kind: NEW_MESSAGE }, actions: [{ commandId: \"chat.echo\" }] }) { id } }",
    )
    .await;

    n.refused(
        "query($s: ID!) { workspaceState(serverId: $s, key: \"poll\", scope: SERVER) { key } }",
    )
    .await;
    n.refused(
        "query($s: ID!) { workspaceStateList(serverId: $s, prefix: \"poll\", scope: SERVER) { key } }",
    )
    .await;
    n.refused(
        "mutation($s: ID!) { setWorkspaceState(input: { serverId: $s, key: \"poll\", \
         value: 1, scope: SERVER }) { key } }",
    )
    .await;
    n.refused(
        "mutation($s: ID!) { deleteWorkspaceState(input: { serverId: $s, key: \"poll\", \
         scope: SERVER }) }",
    )
    .await;
    n.joined_nothing().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_search_is_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { searchMessages(input: { serverId: $s, query: \"hello\" }) { items { id } } }")
        .await;
    assert_eq!(data["searchMessages"]["items"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_changes_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { messageChanges(input: { serverId: $s, conversations: [] }) { conversationId } }")
        .await;
    assert_eq!(data["messageChanges"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_events_subscribe_on_first_use() {
    let n = Newcomer::new().await;
    let user = user_id(&n.node, &n.person.principal()).await;
    let mut stream = n.node.schema.execute_stream(
        Request::new(format!(
            "subscription {{ workspaceEvents(workspaceId: \"{}\") }}",
            n.direct
        ))
        .data(AuthActorCtx::new(Some(user), None)),
    );
    // A refused subscription answers with an error at once; an accepted one waits for events.
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
    assert!(first.is_err(), "the subscription answered: {first:?}");
    drop(stream);
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_between_two_newcomers_makes_both_guests() {
    let n = Newcomer::new().await;
    let other = sign_up(&n.node, "other", 2).await;
    assert_eq!(n.role_of(&other).await, None);
    let created = n
        .person
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": n.direct,
                "encrypted": true,
                "remoteMembers": [{ "principalId": other.principal(), "relayUrls": [n.node.origin] }],
            } }),
        )
        .await;
    assert!(created["createConversation"]["id"].is_string());
    assert_eq!(n.role_of(&other).await.as_deref(), Some("guest"));
    n.joined_as_guest().await;
}

/// The caller's own principal is neither a contact nor a DM partner; a group naming the caller
/// among others is still created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_newcomer_cannot_add_themselves_or_start_a_dm_with_themselves() {
    let n = Newcomer::new().await;
    let me = json!({ "principalId": n.person.principal(), "relayUrls": [n.node.origin] });
    let refused = |body: Value, message: &str| {
        let error = &body["errors"][0];
        assert_eq!(error["extensions"]["code"], "VALIDATION", "{body}");
        assert_eq!(error["message"], message, "{body}");
    };

    let body = n
        .person
        .post(
            "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
            json!({ "p": n.person.principal(), "r": [n.node.origin] }),
        )
        .await;
    refused(body, "cannot add yourself as a contact");
    let contacts = n
        .person
        .ok("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(contacts["contacts"], json!([]));

    let body = n
        .person
        .post(
            CREATE,
            json!({ "input": { "serverId": n.direct, "encrypted": true, "remoteMembers": [me] } }),
        )
        .await;
    refused(body, "cannot create DM with yourself");

    let other = sign_up(&n.node, "other", 2).await;
    let created = n
        .person
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": n.direct,
                "encrypted": true,
                "remoteMembers": [me, { "principalId": other.principal(), "relayUrls": [n.node.origin] }],
            } }),
        )
        .await;
    assert!(created["createConversation"]["id"].is_string());
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_package_roots_answer_without_joining_the_direct_space() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query { myMlsKeyPackageStatus { available } myMlsWelcomes { id } }")
        .await;
    assert!(data["myMlsKeyPackageStatus"][0]["available"].as_i64() > Some(0));
    assert_eq!(data["myMlsWelcomes"], json!([]));
    assert_eq!(n.role_of(&n.person).await, None);
    n.node.stop().await;
}
