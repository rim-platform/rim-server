//! Through-the-stack e2e tests for the channel lens roots: `channelLenses`, `applyChannelLens`,
//! `createChannelLens`, `updateChannelLens` and `deleteChannelLens`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};

const LENS_FIELDS: &str = "id serverId conversationId createdBy name \
     filterSpec { conditions { field op value } logic } isShared createdAt updatedAt";

fn create_mutation() -> String {
    format!(
        "mutation Create($input: CreateChannelLensInput!) {{ lens: createChannelLens(input: $input) {{ {LENS_FIELDS} }} }}"
    )
}

fn update_mutation() -> String {
    format!(
        "mutation Update($input: UpdateChannelLensInput!) {{ lens: updateChannelLens(input: $input) {{ {LENS_FIELDS} }} }}"
    )
}

const DELETE_MUTATION: &str = r#"
mutation Delete($serverId: ID!, $lensId: ID!) {
  deleted: deleteChannelLens(serverId: $serverId, lensId: $lensId)
}
"#;

const LIST_QUERY: &str = r#"
query List($serverId: ID!, $conversationId: ID!) {
  lenses: channelLenses(serverId: $serverId, conversationId: $conversationId) { id name isShared }
}
"#;

const APPLY_QUERY: &str = r#"
query Apply($serverId: ID!, $conversationId: ID!, $lensId: ID!, $limit: Int! = 50) {
  messages: applyChannelLens(serverId: $serverId, conversationId: $conversationId,
                             lensId: $lensId, limit: $limit) {
    id serverId conversationId authorId text hasAttachments isDeleted sentAt
  }
}
"#;

const CREATE_CHANNEL: &str = r#"
mutation Create($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  conversation: addConversationMembers(serverId: $serverId, conversationId: $conversationId,
                                       userIds: $userIds) { id }
}
"#;

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }
"#;

const DELETE_MESSAGE: &str = r#"
mutation Delete($input: DeleteMessageInput!) { deleteMessage(input: $input) }
"#;

const PIN_MESSAGE: &str = r#"
mutation Pin($input: PinMessageInput!) { pinMessage(input: $input) }
"#;

struct Fixture {
    schema: AppSchema,
    server_id: String,
    /// A second workspace alice and bob both belong to.
    other_server_id: String,
    /// Owner; created every channel.
    alice: Caller,
    /// Member.
    bob: Caller,
    /// Guest; in no channel.
    gina: Caller,
    /// Not a member of the workspace.
    olive: Caller,
    /// Public channel.
    general: String,
    /// Second public channel.
    random: String,
    /// Private channel only alice is in.
    secret: String,
    /// Public channel of the second workspace.
    elsewhere: String,
}

/// alice (Owner), bob (Member), gina (Guest) in a workspace with two public channels and a
/// private one; alice and bob also share a second workspace; olive belongs to neither.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("lens-e2e").await;
    let other = ctx.create_server("lens-e2e-other").await;
    let alice_user = ctx.create_user("lens-alice").await;
    let bob_user = ctx.create_user("lens-bob").await;
    let gina_user = ctx.create_user("lens-gina").await;
    let olive_user = ctx.create_user("lens-olive").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina_user, ServerMemberRole::Guest)
        .await;
    ctx.add_owner(&other, &alice_user).await;
    ctx.add_member(&other, &bob_user, ServerMemberRole::Member)
        .await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let alice = ctx.caller(&alice_user).await;
    let server_id = server.id.to_string();
    let other_server_id = other.id.to_string();
    let general = create_channel(&schema, &alice, &server_id, "general", false).await;
    let random = create_channel(&schema, &alice, &server_id, "random", false).await;
    let secret = create_channel(&schema, &alice, &server_id, "secret", true).await;
    let elsewhere = create_channel(&schema, &alice, &other_server_id, "elsewhere", false).await;

    Fixture {
        bob: ctx.caller(&bob_user).await,
        gina: ctx.caller(&gina_user).await,
        olive: ctx.caller(&olive_user).await,
        alice,
        schema,
        server_id,
        other_server_id,
        general,
        random,
        secret,
        elsewhere,
    }
}

async fn create_channel(
    schema: &AppSchema,
    who: &Caller,
    server_id: &str,
    title: &str,
    is_private: bool,
) -> String {
    let data = exec_ok(
        schema,
        who.request(
            CREATE_CHANNEL,
            json!({ "input": {
                "serverId": server_id,
                "isChannel": true,
                "title": title,
                "isPrivate": is_private,
            } }),
        ),
    )
    .await;
    data["conversation"]["id"]
        .as_str()
        .expect("channel id")
        .to_string()
}

fn text_contains(pattern: &str) -> Value {
    json!({ "conditions": [{ "field": "TEXT_CONTAINS", "op": "CONTAINS", "value": pattern }],
            "logic": "AND" })
}

impl Fixture {
    fn create_request(
        &self,
        who: &Caller,
        conversation_id: &str,
        name: &str,
        filter_spec: Value,
        is_shared: Option<bool>,
    ) -> async_graphql::Request {
        who.request(
            &create_mutation(),
            json!({ "input": {
                "serverId": self.server_id,
                "conversationId": conversation_id,
                "name": name,
                "filterSpec": filter_spec,
                "isShared": is_shared,
            } }),
        )
    }

    async fn create(
        &self,
        who: &Caller,
        conversation_id: &str,
        name: &str,
        filter_spec: Value,
        is_shared: bool,
    ) -> Value {
        let data = exec_ok(
            &self.schema,
            self.create_request(who, conversation_id, name, filter_spec, Some(is_shared)),
        )
        .await;
        data["lens"].clone()
    }

    async fn create_err(
        &self,
        who: &Caller,
        conversation_id: &str,
        name: &str,
        spec: Value,
    ) -> String {
        exec_err_code(
            &self.schema,
            self.create_request(who, conversation_id, name, spec, None),
        )
        .await
    }

    fn update_request(
        &self,
        who: &Caller,
        server_id: &str,
        lens_id: &str,
        changes: Value,
    ) -> async_graphql::Request {
        let mut input = json!({ "serverId": server_id, "lensId": lens_id });
        for (k, v) in changes.as_object().expect("changes").iter() {
            input[k] = v.clone();
        }
        who.request(&update_mutation(), json!({ "input": input }))
    }

    fn delete_request(
        &self,
        who: &Caller,
        server_id: &str,
        lens_id: &str,
    ) -> async_graphql::Request {
        who.request(
            DELETE_MUTATION,
            json!({ "serverId": server_id, "lensId": lens_id }),
        )
    }

    fn list_request(&self, who: &Caller, conversation_id: &str) -> async_graphql::Request {
        who.request(
            LIST_QUERY,
            json!({ "serverId": self.server_id, "conversationId": conversation_id }),
        )
    }

    async fn list_names(&self, who: &Caller, conversation_id: &str) -> Vec<String> {
        let data = exec_ok(&self.schema, self.list_request(who, conversation_id)).await;
        let mut names: Vec<String> = data["lenses"]
            .as_array()
            .expect("lenses")
            .iter()
            .map(|l| l["name"].as_str().expect("name").to_string())
            .collect();
        names.sort();
        names
    }

    fn apply_request(
        &self,
        who: &Caller,
        conversation_id: &str,
        lens_id: &str,
        limit: Option<i32>,
    ) -> async_graphql::Request {
        let mut vars = json!({
            "serverId": self.server_id,
            "conversationId": conversation_id,
            "lensId": lens_id,
        });
        if let Some(limit) = limit {
            vars["limit"] = json!(limit);
        }
        who.request(APPLY_QUERY, vars)
    }

    async fn apply(&self, who: &Caller, lens_id: &str, limit: Option<i32>) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            self.apply_request(who, &self.general, lens_id, limit),
        )
        .await;
        data["messages"].as_array().expect("messages").clone()
    }

    async fn apply_texts(&self, who: &Caller, lens_id: &str) -> Vec<String> {
        self.apply(who, lens_id, None)
            .await
            .iter()
            .map(|m| m["text"].as_str().expect("text").to_string())
            .collect()
    }

    async fn send(&self, who: &Caller, text: &str) -> String {
        let data = exec_ok(
            &self.schema,
            who.request(
                SEND_MESSAGE,
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": self.general,
                    "text": text,
                } }),
            ),
        )
        .await;
        data["msg"]["id"].as_str().expect("message id").to_string()
    }

    async fn message_op(&self, mutation: &str, message_id: &str) {
        exec_ok(
            &self.schema,
            self.alice.request(
                mutation,
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": self.general,
                    "messageId": message_id,
                } }),
            ),
        )
        .await;
    }
}

fn id_of(lens: &Value) -> String {
    lens["id"].as_str().expect("lens id").to_string()
}

// ── Authentication and membership ─────────────────────────────────────────

#[tokio::test]
async fn every_channel_lens_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "links", text_contains("x"), true)
            .await,
    );
    let requests = [
        anonymous_request(
            LIST_QUERY,
            json!({ "serverId": f.server_id, "conversationId": f.general }),
        ),
        anonymous_request(
            APPLY_QUERY,
            json!({ "serverId": f.server_id, "conversationId": f.general, "lensId": lens }),
        ),
        anonymous_request(
            &create_mutation(),
            json!({ "input": { "serverId": f.server_id, "conversationId": f.general,
                               "name": "n", "filterSpec": text_contains("x") } }),
        ),
        anonymous_request(
            &update_mutation(),
            json!({ "input": { "serverId": f.server_id, "lensId": lens, "name": "n" } }),
        ),
        anonymous_request(
            DELETE_MUTATION,
            json!({ "serverId": f.server_id, "lensId": lens }),
        ),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "UNAUTHENTICATED");
    }
}

#[tokio::test]
async fn every_channel_lens_root_refuses_a_non_member() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "links", text_contains("x"), true)
            .await,
    );
    let requests = [
        f.list_request(&f.olive, &f.general),
        f.apply_request(&f.olive, &f.general, &lens, None),
        f.create_request(&f.olive, &f.general, "n", text_contains("x"), None),
        f.update_request(&f.olive, &f.server_id, &lens, json!({ "name": "n" })),
        f.delete_request(&f.olive, &f.server_id, &lens),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "FORBIDDEN");
    }
}

#[tokio::test]
async fn a_guest_uses_lenses_only_in_channels_they_were_added_to() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "shared", text_contains("x"), true)
            .await,
    );

    assert_eq!(
        f.create_err(&f.gina, &f.general, "mine", text_contains("x"))
            .await,
        "NOT_FOUND"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.list_request(&f.gina, &f.general)).await,
        "NOT_FOUND"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.apply_request(&f.gina, &f.general, &lens, None)).await,
        "NOT_FOUND"
    );

    exec_ok(
        &f.schema,
        f.alice.request(
            ADD_MEMBERS,
            json!({ "serverId": f.server_id, "conversationId": f.general,
                    "userIds": [f.gina.user_id.to_string()] }),
        ),
    )
    .await;
    f.create(&f.gina, &f.general, "mine", text_contains("x"), false)
        .await;
    assert_eq!(f.list_names(&f.gina, &f.general).await, ["mine", "shared"]);
    assert!(f.apply(&f.gina, &lens, None).await.is_empty());
}

#[tokio::test]
async fn a_member_outside_a_private_channel_gets_not_found_for_its_lenses() {
    let f = setup().await;
    assert_eq!(
        f.create_err(&f.bob, &f.secret, "mine", text_contains("x"))
            .await,
        "NOT_FOUND"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.list_request(&f.bob, &f.secret)).await,
        "NOT_FOUND"
    );
    f.create(&f.alice, &f.secret, "inside", text_contains("x"), false)
        .await;
}

// ── Create ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_channel_lens_returns_the_lens_with_its_filter_spec() {
    let f = setup().await;
    let spec = json!({
        "conditions": [
            { "field": "HAS_LINKS", "op": "EQ", "value": true },
            { "field": "AUTHOR_ID", "op": "IN", "value": [f.bob.user_id.to_string()] },
            { "field": "HAS_ATTACHMENTS", "op": "EQ", "value": false },
            { "field": "TEXT_CONTAINS", "op": "CONTAINS", "value": "release" },
            { "field": "IS_PINNED", "op": "EQ", "value": true },
        ],
        "logic": "OR",
    });
    let data = exec_ok(
        &f.schema,
        f.create_request(&f.alice, &f.general, "Release notes", spec.clone(), None),
    )
    .await;
    let lens = &data["lens"];

    assert!(lens["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(lens["serverId"], f.server_id.as_str());
    assert_eq!(lens["conversationId"], f.general.as_str());
    assert_eq!(lens["createdBy"], f.alice.user_id.to_string().as_str());
    assert_eq!(lens["name"], "Release notes");
    assert_eq!(lens["filterSpec"], spec);
    assert_eq!(
        lens["isShared"], false,
        "a lens is private unless asked otherwise"
    );
    assert!(lens["createdAt"].is_string());
    assert_eq!(lens["createdAt"], lens["updatedAt"]);
}

#[tokio::test]
async fn create_channel_lens_refuses_a_conversation_outside_the_workspace() {
    let f = setup().await;
    let unknown = uuid::Uuid::now_v7().to_string();
    for conversation in [unknown.as_str(), f.elsewhere.as_str()] {
        assert_eq!(
            f.create_err(&f.alice, conversation, "n", text_contains("x"))
                .await,
            "NOT_FOUND"
        );
    }
}

#[tokio::test]
async fn create_channel_lens_takes_a_name_of_one_to_a_hundred_characters() {
    let f = setup().await;
    for name in [String::new(), "é".repeat(101)] {
        assert_eq!(
            f.create_err(&f.alice, &f.general, &name, text_contains("x"))
                .await,
            "VALIDATION",
            "name of {} characters",
            name.chars().count()
        );
    }
    f.create(
        &f.alice,
        &f.general,
        &"é".repeat(100),
        text_contains("x"),
        false,
    )
    .await;
}

#[tokio::test]
async fn create_channel_lens_takes_one_to_twenty_conditions() {
    let f = setup().await;
    let spec = |n: usize| {
        let conditions: Vec<Value> = (0..n)
            .map(
                |i| json!({ "field": "TEXT_CONTAINS", "op": "CONTAINS", "value": format!("w{i}") }),
            )
            .collect();
        json!({ "conditions": conditions, "logic": "OR" })
    };
    for n in [0, 21] {
        assert_eq!(
            f.create_err(&f.alice, &f.general, "n", spec(n)).await,
            "VALIDATION",
            "{n} conditions"
        );
    }
    f.create(&f.alice, &f.general, "twenty", spec(20), false)
        .await;
}

#[tokio::test]
async fn a_member_owns_at_most_ten_lenses_per_conversation() {
    let f = setup().await;
    for i in 0..10 {
        f.create(
            &f.alice,
            &f.general,
            &format!("lens {i}"),
            text_contains("x"),
            false,
        )
        .await;
    }
    assert_eq!(
        f.create_err(&f.alice, &f.general, "lens 10", text_contains("x"))
            .await,
        "VALIDATION"
    );
    f.create(
        &f.alice,
        &f.random,
        "other channel",
        text_contains("x"),
        false,
    )
    .await;
    f.create(
        &f.bob,
        &f.general,
        "other member",
        text_contains("x"),
        false,
    )
    .await;
}

// ── List ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn channel_lenses_lists_your_own_lenses_and_the_shared_ones_of_the_conversation() {
    let f = setup().await;
    f.create(
        &f.alice,
        &f.general,
        "alice private",
        text_contains("x"),
        false,
    )
    .await;
    f.create(
        &f.alice,
        &f.general,
        "alice shared",
        text_contains("x"),
        true,
    )
    .await;
    f.create(&f.bob, &f.general, "bob private", text_contains("x"), false)
        .await;
    f.create(
        &f.alice,
        &f.random,
        "alice random",
        text_contains("x"),
        true,
    )
    .await;

    assert_eq!(
        f.list_names(&f.bob, &f.general).await,
        ["alice shared", "bob private"]
    );
    assert_eq!(
        f.list_names(&f.alice, &f.general).await,
        ["alice private", "alice shared"]
    );
}

// ── Update ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_channel_lens_changes_only_the_fields_given() {
    let f = setup().await;
    let created = f
        .create(&f.alice, &f.general, "before", text_contains("x"), false)
        .await;
    let lens = id_of(&created);

    let data = exec_ok(
        &f.schema,
        f.update_request(&f.alice, &f.server_id, &lens, json!({ "name": "after" })),
    )
    .await;
    let renamed = &data["lens"];
    assert_eq!(renamed["id"], created["id"]);
    assert_eq!(renamed["name"], "after");
    assert_eq!(renamed["filterSpec"], created["filterSpec"]);
    assert_eq!(renamed["isShared"], false);
    assert_eq!(renamed["createdAt"], created["createdAt"]);
    assert_ne!(renamed["updatedAt"], created["updatedAt"]);

    let new_spec = json!({ "conditions": [{ "field": "HAS_LINKS", "op": "EQ", "value": true }],
                           "logic": "AND" });
    let data = exec_ok(
        &f.schema,
        f.update_request(
            &f.alice,
            &f.server_id,
            &lens,
            json!({ "filterSpec": new_spec, "isShared": true }),
        ),
    )
    .await;
    assert_eq!(data["lens"]["name"], "after");
    assert_eq!(data["lens"]["filterSpec"], new_spec);
    assert_eq!(data["lens"]["isShared"], true);
    assert_eq!(f.list_names(&f.bob, &f.general).await, ["after"]);
}

#[tokio::test]
async fn update_channel_lens_refuses_anyone_but_its_creator() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "mine", text_contains("x"), true)
            .await,
    );
    let code = exec_err_code(
        &f.schema,
        f.update_request(&f.bob, &f.server_id, &lens, json!({ "name": "taken" })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");
    assert_eq!(f.list_names(&f.alice, &f.general).await, ["mine"]);
}

#[tokio::test]
async fn update_channel_lens_gets_not_found_for_an_unknown_lens_or_another_workspace() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "mine", text_contains("x"), false)
            .await,
    );
    let unknown = uuid::Uuid::now_v7().to_string();
    let requests = [
        f.update_request(&f.alice, &f.server_id, &unknown, json!({ "name": "n" })),
        f.update_request(&f.alice, &f.other_server_id, &lens, json!({ "name": "n" })),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "NOT_FOUND");
    }
}

#[tokio::test]
async fn update_channel_lens_validates_the_new_name_and_filter() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "mine", text_contains("x"), false)
            .await,
    );
    let changes = [
        json!({ "name": "" }),
        json!({ "name": "n".repeat(101) }),
        json!({ "filterSpec": { "conditions": [], "logic": "AND" } }),
    ];
    for change in changes {
        assert_eq!(
            exec_err_code(
                &f.schema,
                f.update_request(&f.alice, &f.server_id, &lens, change.clone())
            )
            .await,
            "VALIDATION",
            "{change}"
        );
    }
    assert_eq!(f.list_names(&f.alice, &f.general).await, ["mine"]);
}

// ── Delete ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_channel_lens_removes_the_creators_lens() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "mine", text_contains("x"), true)
            .await,
    );

    assert_eq!(
        exec_err_code(&f.schema, f.delete_request(&f.bob, &f.server_id, &lens)).await,
        "FORBIDDEN"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.delete_request(&f.alice, &f.other_server_id, &lens)
        )
        .await,
        "NOT_FOUND"
    );

    let data = exec_ok(&f.schema, f.delete_request(&f.alice, &f.server_id, &lens)).await;
    assert_eq!(data["deleted"], true);
    assert!(f.list_names(&f.alice, &f.general).await.is_empty());
    assert_eq!(
        exec_err_code(&f.schema, f.delete_request(&f.alice, &f.server_id, &lens)).await,
        "NOT_FOUND"
    );
}

// ── Apply ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn apply_channel_lens_returns_matching_messages_newest_first() {
    let f = setup().await;
    let first = f.send(&f.alice, "Release v1 is out").await;
    f.send(&f.bob, "lunch?").await;
    f.send(&f.bob, "the RELEASE notes are long").await;
    let lens = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "release",
            text_contains("release"),
            false,
        )
        .await,
    );

    let messages = f.apply(&f.alice, &lens, None).await;
    let texts: Vec<&str> = messages
        .iter()
        .map(|m| m["text"].as_str().expect("text"))
        .collect();
    assert_eq!(texts, ["the RELEASE notes are long", "Release v1 is out"]);

    let oldest = &messages[1];
    assert_eq!(oldest["id"], first.as_str());
    assert_eq!(oldest["serverId"], f.server_id.as_str());
    assert_eq!(oldest["conversationId"], f.general.as_str());
    assert_eq!(oldest["authorId"], f.alice.user_id.to_string().as_str());
    assert_eq!(oldest["hasAttachments"], false);
    assert_eq!(oldest["isDeleted"], false);
    assert!(oldest["sentAt"].is_string());
}

#[tokio::test]
async fn apply_channel_lens_combines_conditions_with_and_or_or() {
    let f = setup().await;
    f.send(&f.alice, "see https://example.com").await;
    f.send(&f.bob, "see http://example.org").await;
    f.send(&f.bob, "no link here").await;
    let bob_id = f.bob.user_id.to_string();
    let conditions = json!([
        { "field": "HAS_LINKS", "op": "EQ", "value": true },
        { "field": "AUTHOR_ID", "op": "EQ", "value": bob_id },
    ]);

    let and = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "and",
            json!({ "conditions": conditions, "logic": "AND" }),
            false,
        )
        .await,
    );
    assert_eq!(
        f.apply_texts(&f.alice, &and).await,
        ["see http://example.org"]
    );

    let or = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "or",
            json!({ "conditions": conditions, "logic": "OR" }),
            false,
        )
        .await,
    );
    assert_eq!(
        f.apply_texts(&f.alice, &or).await,
        [
            "no link here",
            "see http://example.org",
            "see https://example.com"
        ]
    );

    let no_links = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "no links",
            json!({ "conditions": [{ "field": "HAS_LINKS", "op": "EQ", "value": false }],
                    "logic": "AND" }),
            false,
        )
        .await,
    );
    assert_eq!(f.apply_texts(&f.alice, &no_links).await, ["no link here"]);

    let authors = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "authors",
            json!({ "conditions": [{ "field": "AUTHOR_ID", "op": "IN",
                                     "value": [f.alice.user_id.to_string()] }],
                    "logic": "AND" }),
            false,
        )
        .await,
    );
    assert_eq!(
        f.apply_texts(&f.alice, &authors).await,
        ["see https://example.com"]
    );
}

#[tokio::test]
async fn apply_channel_lens_matches_pinned_messages_and_skips_deleted_ones() {
    let f = setup().await;
    let pinned = f.send(&f.alice, "pinned one").await;
    f.send(&f.alice, "plain one").await;
    let deleted = f.send(&f.alice, "deleted one").await;
    f.message_op(PIN_MESSAGE, &pinned).await;
    f.message_op(DELETE_MESSAGE, &deleted).await;

    let pins = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "pins",
            json!({ "conditions": [{ "field": "IS_PINNED", "op": "EQ", "value": true }],
                    "logic": "AND" }),
            false,
        )
        .await,
    );
    assert_eq!(f.apply_texts(&f.alice, &pins).await, ["pinned one"]);

    let all = id_of(
        &f.create(&f.alice, &f.general, "all", text_contains("one"), false)
            .await,
    );
    assert_eq!(
        f.apply_texts(&f.alice, &all).await,
        ["plain one", "pinned one"]
    );
}

#[tokio::test]
async fn apply_channel_lens_returns_between_one_and_fifty_messages() {
    let f = setup().await;
    for i in 0..51 {
        f.send(&f.bob, &format!("match {i}")).await;
    }
    let lens = id_of(
        &f.create(&f.alice, &f.general, "all", text_contains("match"), false)
            .await,
    );

    assert_eq!(f.apply(&f.alice, &lens, None).await.len(), 50);
    assert_eq!(f.apply(&f.alice, &lens, Some(500)).await.len(), 50);
    assert_eq!(f.apply(&f.alice, &lens, Some(3)).await.len(), 3);
    let one = f.apply(&f.alice, &lens, Some(0)).await;
    assert_eq!(one.len(), 1);
    assert_eq!(one[0]["text"], "match 50");
}

#[tokio::test]
async fn apply_channel_lens_runs_another_members_lens_only_when_shared() {
    let f = setup().await;
    f.send(&f.alice, "hello").await;
    let private = id_of(
        &f.create(
            &f.alice,
            &f.general,
            "private",
            text_contains("hello"),
            false,
        )
        .await,
    );
    let shared = id_of(
        &f.create(&f.alice, &f.general, "shared", text_contains("hello"), true)
            .await,
    );

    assert_eq!(
        exec_err_code(
            &f.schema,
            f.apply_request(&f.bob, &f.general, &private, None)
        )
        .await,
        "FORBIDDEN"
    );
    assert_eq!(f.apply_texts(&f.bob, &shared).await, ["hello"]);
}

#[tokio::test]
async fn apply_channel_lens_gets_not_found_for_an_unknown_lens_or_another_conversation() {
    let f = setup().await;
    let lens = id_of(
        &f.create(&f.alice, &f.general, "mine", text_contains("x"), false)
            .await,
    );
    let unknown = uuid::Uuid::now_v7().to_string();
    let requests = [
        f.apply_request(&f.alice, &f.general, &unknown, None),
        f.apply_request(&f.alice, &f.random, &lens, None),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "NOT_FOUND");
    }
}
