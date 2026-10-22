//! What a member may do to the shared surfaces of the server's default workspace, over a real
//! database: a channel's topic and others' pins are its Owner's and Moderators' (or a private
//! channel creator's), only they add people to a channel and notify everyone with `@channel`,
//! and a group is started and grown with at most 50 people.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test default_workspace_rules_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, exec_err, exec_ok};
use support::roles::Roles;

const CREATE_CHANNEL: &str = "mutation($s: ID!, $title: String!, $private: Boolean) { \
     createConversation(input: { serverId: $s, isChannel: true, title: $title, \
     isPrivate: $private }) { id } }";
const CREATE_GROUP: &str = "mutation($s: ID!, $members: [ID!]!) { \
     createConversation(input: { serverId: $s, memberIds: $members }) { id } }";
const JOIN: &str =
    "mutation($s: ID!, $c: ID!) { joinChannel(serverId: $s, conversationId: $c) { id } }";
const ADD: &str = "mutation($s: ID!, $c: ID!, $u: [ID!]!) { \
     addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { id } }";
const TOPIC: &str = "mutation($s: ID!, $c: ID!, $t: String) { \
     updateConversationTopic(input: { serverId: $s, conversationId: $c, topic: $t }) { id } }";
const SEND: &str = "mutation($s: ID!, $c: ID!, $text: String!) { \
     sendMessage(input: { serverId: $s, conversationId: $c, text: $text }) { id } }";
const PIN: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { \
     pinMessage(input: { serverId: $s, conversationId: $c, messageId: $m }) }";
const UNPIN: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { \
     unpinMessage(input: { serverId: $s, conversationId: $c, messageId: $m }) }";
const SET_LEVEL: &str = "mutation($s: ID!, $c: ID!, $l: NotificationLevel) { \
     setConversationNotificationLevel(serverId: $s, conversationId: $c, level: $l) { level } }";
const NOTIFICATIONS: &str =
    "query($s: ID!) { myNotifications(serverId: $s, limit: 50) { items { messageId reason } } }";

struct Fixture {
    schema: AppSchema,
    ws: Roles,
    /// A public channel alice created in the default workspace; bob and carol joined it.
    news: String,
}

/// The roles fixture's workspace as the server's default workspace, with a public channel.
async fn setup(label: &str) -> (TestCtx, Fixture) {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles(label).await;
    let default_id = ws
        .server_id
        .parse::<uuid::Uuid>()
        .expect("server id")
        .into();
    let schema = ctx.build_schema_with_default_workspace(default_id, RegistrationMode::InviteOnly);
    let news = id(&exec_ok(
        &schema,
        ws.alice.request(
            CREATE_CHANNEL,
            json!({ "s": ws.server_id, "title": "news", "private": false }),
        ),
    )
    .await["createConversation"]);
    for who in [&ws.bob, &ws.carol, &ws.mona] {
        exec_ok(
            &schema,
            who.request(JOIN, json!({ "s": ws.server_id, "c": news })),
        )
        .await;
    }
    (ctx, Fixture { schema, ws, news })
}

fn id(value: &Value) -> String {
    value["id"].as_str().expect("id").to_string()
}

impl Fixture {
    async fn send(&self, who: &Caller, text: &str) -> String {
        id(&exec_ok(
            &self.schema,
            who.request(
                SEND,
                json!({ "s": self.ws.server_id, "c": self.news, "text": text }),
            ),
        )
        .await["sendMessage"])
    }

    fn pin(&self, who: &Caller, query: &str, message: &str) -> async_graphql::Request {
        who.request(
            query,
            json!({ "s": self.ws.server_id, "c": self.news, "m": message }),
        )
    }
}

#[tokio::test]
async fn a_channel_topic_and_others_pins_are_the_staffs() {
    let (_ctx, f) = setup("dw-topic").await;
    let topic = |who: &Caller| {
        who.request(
            TOPIC,
            json!({ "s": f.ws.server_id, "c": f.news, "t": "mine now" }),
        )
    };
    let (code, message) = exec_err(&f.schema, topic(&f.ws.bob)).await;
    assert_eq!(code, "FORBIDDEN");
    assert_eq!(
        message,
        "only the owner and moderators change a channel's topic"
    );
    exec_ok(&f.schema, topic(&f.ws.mona)).await;

    let alices = f.send(&f.ws.alice, "read this first").await;
    exec_ok(&f.schema, f.pin(&f.ws.alice, PIN, &alices)).await;
    let (code, _) = exec_err(&f.schema, f.pin(&f.ws.bob, UNPIN, &alices)).await;
    assert_eq!(code, "FORBIDDEN");
    let bobs = f.send(&f.ws.bob, "and this").await;
    exec_ok(&f.schema, f.pin(&f.ws.bob, PIN, &bobs)).await;
    exec_ok(&f.schema, f.pin(&f.ws.bob, UNPIN, &bobs)).await;
    exec_ok(&f.schema, f.pin(&f.ws.mona, UNPIN, &alices)).await;
}

#[tokio::test]
async fn only_staff_and_a_private_channels_creator_add_people_to_a_channel() {
    let (_ctx, f) = setup("dw-add").await;
    let add = |who: &Caller, channel: &str, user: &Caller| {
        who.request(
            ADD,
            json!({ "s": f.ws.server_id, "c": channel, "u": [user.user_id.to_string()] }),
        )
    };
    let (code, _) = exec_err(&f.schema, add(&f.ws.bob, &f.news, &f.ws.gina)).await;
    assert_eq!(code, "FORBIDDEN");
    exec_ok(&f.schema, add(&f.ws.mona, &f.news, &f.ws.gina)).await;

    let staff_room = id(&exec_ok(
        &f.schema,
        f.ws.mona.request(
            CREATE_CHANNEL,
            json!({ "s": f.ws.server_id, "title": "staff", "private": true }),
        ),
    )
    .await["createConversation"]);
    exec_ok(&f.schema, add(&f.ws.mona, &staff_room, &f.ws.bob)).await;
    let (code, _) = exec_err(&f.schema, add(&f.ws.bob, &staff_room, &f.ws.carol)).await;
    assert_eq!(
        code, "FORBIDDEN",
        "only its creator adds to a private channel"
    );
    exec_ok(&f.schema, add(&f.ws.mona, &staff_room, &f.ws.carol)).await;
}

#[tokio::test]
async fn a_group_takes_at_most_fifty_people_per_call() {
    let (_ctx, f) = setup("dw-group").await;
    let strangers: Vec<String> = (0..51).map(|_| uuid::Uuid::now_v7().to_string()).collect();
    let (code, message) = exec_err(
        &f.schema,
        f.ws.bob.request(
            CREATE_GROUP,
            json!({ "s": f.ws.server_id, "members": strangers }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("at most 50 people"), "{message}");

    let group = id(&exec_ok(
        &f.schema,
        f.ws.bob.request(
            CREATE_GROUP,
            json!({ "s": f.ws.server_id, "members": [f.ws.carol.user_id.to_string()] }),
        ),
    )
    .await["createConversation"]);
    let (code, message) = exec_err(
        &f.schema,
        f.ws.bob.request(
            ADD,
            json!({ "s": f.ws.server_id, "c": group, "u": strangers }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("at most 50 people"), "{message}");
}

#[tokio::test]
async fn only_the_staffs_channel_mentions_notify_everyone() {
    let (_ctx, f) = setup("dw-channel").await;
    exec_ok(
        &f.schema,
        f.ws.carol.request(
            SET_LEVEL,
            json!({ "s": f.ws.server_id, "c": f.news, "l": "MENTIONS" }),
        ),
    )
    .await;
    let from_bob = f.send(&f.ws.bob, "@channel free stuff").await;
    let from_alice = f.send(&f.ws.alice, "@here standup in 5").await;

    let carols = exec_ok(
        &f.schema,
        f.ws.carol
            .request(NOTIFICATIONS, json!({ "s": f.ws.server_id })),
    )
    .await;
    let items = carols["myNotifications"]["items"]
        .as_array()
        .expect("items")
        .clone();
    assert_eq!(
        items,
        vec![json!({ "messageId": from_alice, "reason": "MENTION" })],
        "bob's @channel is plain text: {from_bob}"
    );
}
