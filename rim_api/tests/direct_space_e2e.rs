//! The roots a client sends against the direct space, each called by an account that has never
//! joined anything and never read `relayInfo` with its session. A workspace-scoped root is a
//! first use: it makes the caller a guest of the direct space and answers as it does for any
//! guest, an empty list where they have nothing yet. Roots that need no membership join nobody.
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
async fn channels_are_empty_browsed_or_joined_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { \
             browse: channels(serverId: $s, dir: AFTER, limit: 100) { items { id } } \
             joined: channels(serverId: $s, dir: AFTER, limit: 100, filter: { joinedOnly: true }) { items { id } } }")
        .await;
    assert_eq!(data["browse"]["items"], json!([]));
    assert_eq!(data["joined"]["items"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolved_keybinds_answer_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { resolvedKeybinds(workspaceId: $s) { workspaceId } }")
        .await;
    assert!(
        data["resolvedKeybinds"]
            .as_array()
            .expect("keybinds")
            .iter()
            .all(|k| k["workspaceId"].is_null()),
        "the direct space has no keybinds of its own: {data}"
    );
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_list_without_joining_the_direct_space() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { commands(commandContext: { workspaceId: $s }) { id } }")
        .await;
    assert!(!data["commands"].as_array().expect("commands").is_empty());
    assert_eq!(n.role_of(&n.person).await, None);
    n.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn custom_emojis_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { customEmojis(serverId: $s) { id } }")
        .await;
    assert_eq!(data["customEmojis"], json!([]));
    n.joined_as_guest().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_plugins_are_empty_on_first_use() {
    let n = Newcomer::new().await;
    let data = n
        .ok("query($s: ID!) { workspacePlugins(serverId: $s) { id } }")
        .await;
    assert_eq!(data["workspacePlugins"], json!([]));
    n.joined_as_guest().await;
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
