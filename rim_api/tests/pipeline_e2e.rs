//! Through-the-stack e2e tests for the pipeline roots: `pipelines`, `createPipeline`,
//! `updatePipeline`, `togglePipeline` and `deletePipeline`.
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
use support::graphql::{Caller, anonymous_request, exec_err, exec_err_code, exec_ok};

const PIPELINE_FIELDS: &str = "id serverId name description \
     trigger { kind conversationId } \
     filter { conditions { conditionType pattern userId conversationId } } \
     actions { actionType commandId args } \
     enabled cooldownSecs createdBy schemaVersion lastFiredAt createdAt updatedAt";

fn create_mutation() -> String {
    format!(
        "mutation Create($input: CreatePipelineInput!) {{ pipeline: createPipeline(input: $input) {{ {PIPELINE_FIELDS} }} }}"
    )
}

fn update_mutation() -> String {
    format!(
        "mutation Update($input: UpdatePipelineInput!) {{ pipeline: updatePipeline(input: $input) {{ {PIPELINE_FIELDS} }} }}"
    )
}

fn toggle_mutation() -> String {
    format!(
        "mutation Toggle($pipelineId: ID!, $enabled: Boolean!) {{ pipeline: togglePipeline(pipelineId: $pipelineId, enabled: $enabled) {{ {PIPELINE_FIELDS} }} }}"
    )
}

fn list_query() -> String {
    format!(
        "query List($serverId: ID!, $enabled: Boolean) {{ pipelines: pipelines(serverId: $serverId, enabled: $enabled) {{ {PIPELINE_FIELDS} }} }}"
    )
}

const DELETE_MUTATION: &str = r#"
mutation Delete($pipelineId: ID!) { deleted: deletePipeline(pipelineId: $pipelineId) }
"#;

struct Fixture {
    schema: AppSchema,
    server_id: String,
    other_server_id: String,
    /// Owner.
    alice: Caller,
    /// Moderator.
    mona: Caller,
    /// Member.
    bob: Caller,
    /// Guest.
    gina: Caller,
    /// Not a member of the workspace.
    olive: Caller,
}

/// alice (Owner), mona (Moderator), bob (Member), gina (Guest) in one workspace; olive owns a
/// second one.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("pipeline-e2e").await;
    let other = ctx.create_server("pipeline-e2e-other").await;
    let alice = ctx.create_user("pipe-alice").await;
    let mona = ctx.create_user("pipe-mona").await;
    let bob = ctx.create_user("pipe-bob").await;
    let gina = ctx.create_user("pipe-gina").await;
    let olive = ctx.create_user("pipe-olive").await;
    ctx.add_owner(&server, &alice).await;
    ctx.add_member(&server, &mona, ServerMemberRole::Moderator)
        .await;
    ctx.add_member(&server, &bob, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina, ServerMemberRole::Guest)
        .await;
    ctx.add_owner(&other, &olive).await;

    Fixture {
        schema: ctx.build_schema(RegistrationMode::InviteOnly).await,
        server_id: server.id.to_string(),
        other_server_id: other.id.to_string(),
        alice: ctx.caller(&alice).await,
        mona: ctx.caller(&mona).await,
        bob: ctx.caller(&bob).await,
        gina: ctx.caller(&gina).await,
        olive: ctx.caller(&olive).await,
    }
}

/// The smallest valid body of a create or update input: a name, a trigger, one action.
fn minimal_body(name: &str) -> Value {
    json!({
        "name": name,
        "trigger": { "kind": "NEW_MESSAGE" },
        "actions": [{ "commandId": "chat.post_message" }],
    })
}

/// `body` with `changes` merged over it.
fn with(mut body: Value, changes: Value) -> Value {
    for (k, v) in changes.as_object().expect("changes").iter() {
        body[k] = v.clone();
    }
    body
}

fn id_of(pipeline: &Value) -> String {
    pipeline["id"].as_str().expect("pipeline id").to_string()
}

impl Fixture {
    fn create_request(&self, who: &Caller, server_id: &str, body: Value) -> async_graphql::Request {
        let input = with(body, json!({ "serverId": server_id }));
        who.request(&create_mutation(), json!({ "input": input }))
    }

    async fn create(&self, who: &Caller, body: Value) -> Value {
        let data = exec_ok(
            &self.schema,
            self.create_request(who, &self.server_id, body),
        )
        .await;
        data["pipeline"].clone()
    }

    async fn create_err(&self, who: &Caller, body: Value) -> String {
        exec_err_code(
            &self.schema,
            self.create_request(who, &self.server_id, body),
        )
        .await
    }

    fn update_request(
        &self,
        who: &Caller,
        pipeline_id: &str,
        body: Value,
    ) -> async_graphql::Request {
        let input = with(body, json!({ "pipelineId": pipeline_id }));
        who.request(&update_mutation(), json!({ "input": input }))
    }

    fn toggle_request(
        &self,
        who: &Caller,
        pipeline_id: &str,
        enabled: bool,
    ) -> async_graphql::Request {
        who.request(
            &toggle_mutation(),
            json!({ "pipelineId": pipeline_id, "enabled": enabled }),
        )
    }

    fn delete_request(&self, who: &Caller, pipeline_id: &str) -> async_graphql::Request {
        who.request(DELETE_MUTATION, json!({ "pipelineId": pipeline_id }))
    }

    fn list_request(
        &self,
        who: &Caller,
        server_id: &str,
        enabled: Option<bool>,
    ) -> async_graphql::Request {
        who.request(
            &list_query(),
            json!({ "serverId": server_id, "enabled": enabled }),
        )
    }

    async fn list_names(&self, who: &Caller, enabled: Option<bool>) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            self.list_request(who, &self.server_id, enabled),
        )
        .await;
        let mut names: Vec<String> = data["pipelines"]
            .as_array()
            .expect("pipelines")
            .iter()
            .map(|p| p["name"].as_str().expect("name").to_string())
            .collect();
        names.sort();
        names
    }

    async fn list(&self, who: &Caller) -> Vec<Value> {
        let data = exec_ok(&self.schema, self.list_request(who, &self.server_id, None)).await;
        data["pipelines"].as_array().expect("pipelines").clone()
    }
}

// ── Authentication and membership ─────────────────────────────────────────

#[tokio::test]
async fn every_pipeline_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    let id = id_of(&f.create(&f.alice, minimal_body("p")).await);
    let requests = [
        anonymous_request(&list_query(), json!({ "serverId": f.server_id })),
        anonymous_request(
            &create_mutation(),
            json!({ "input": with(minimal_body("p"), json!({ "serverId": f.server_id })) }),
        ),
        anonymous_request(
            &update_mutation(),
            json!({ "input": with(minimal_body("p"), json!({ "pipelineId": id })) }),
        ),
        anonymous_request(
            &toggle_mutation(),
            json!({ "pipelineId": id, "enabled": false }),
        ),
        anonymous_request(DELETE_MUTATION, json!({ "pipelineId": id })),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "UNAUTHENTICATED");
    }
}

#[tokio::test]
async fn every_pipeline_root_refuses_a_non_member() {
    let f = setup().await;
    let id = id_of(&f.create(&f.alice, minimal_body("p")).await);
    let requests = [
        f.list_request(&f.olive, &f.server_id, None),
        f.create_request(&f.olive, &f.server_id, minimal_body("p")),
        f.update_request(&f.olive, &id, minimal_body("taken")),
        f.toggle_request(&f.olive, &id, false),
        f.delete_request(&f.olive, &id),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "FORBIDDEN");
    }
    let listed = f.list(&f.alice).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "p");
    assert_eq!(listed[0]["enabled"], true);
}

#[tokio::test]
async fn a_guest_may_create_a_pipeline() {
    let f = setup().await;
    let created = f.create(&f.gina, minimal_body("guest pipeline")).await;
    assert_eq!(created["createdBy"], f.gina.user_id.to_string().as_str());
    assert_eq!(f.list_names(&f.alice, None).await, ["guest pipeline"]);
}

#[tokio::test]
async fn only_its_creator_or_a_moderator_may_change_a_pipeline() {
    let f = setup().await;
    let id = id_of(&f.create(&f.alice, minimal_body("owner's")).await);
    for request in [
        f.update_request(&f.bob, &id, minimal_body("taken")),
        f.toggle_request(&f.bob, &id, false),
        f.delete_request(&f.bob, &id),
    ] {
        assert_eq!(exec_err_code(&f.schema, request).await, "FORBIDDEN");
    }
    assert_eq!(f.list_names(&f.alice, Some(true)).await, ["owner's"]);

    let data = exec_ok(
        &f.schema,
        f.update_request(&f.mona, &id, minimal_body("moderated")),
    )
    .await;
    assert_eq!(
        data["pipeline"]["createdBy"],
        f.alice.user_id.to_string().as_str()
    );
    let data = exec_ok(&f.schema, f.toggle_request(&f.mona, &id, false)).await;
    assert_eq!(data["pipeline"]["enabled"], false);
    let data = exec_ok(&f.schema, f.delete_request(&f.mona, &id)).await;
    assert_eq!(data["deleted"], true);
}

// ── Create ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_pipeline_returns_an_enabled_pipeline_with_every_field() {
    let f = setup().await;
    let conversation = uuid::Uuid::now_v7().to_string();
    let author = f.bob.user_id.to_string();
    let body = json!({
        "name": "Echo deploys",
        "description": "Repost deploy notices",
        "trigger": { "kind": "MESSAGE_UPDATED", "conversationId": conversation },
        "filter": { "conditions": [
            { "conditionType": "MessageContains", "pattern": "deploy" },
            { "conditionType": "MessageFromUser", "userId": author },
            { "conditionType": "ConversationIs", "conversationId": conversation },
        ] },
        "actions": [
            { "commandId": "chat.post_message", "args": { "text": "seen" } },
            { "commandId": "chat.react" },
        ],
        "cooldownSecs": 30,
    });
    let p = f.create(&f.alice, body).await;

    assert!(p["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(p["serverId"], f.server_id.as_str());
    assert_eq!(p["name"], "Echo deploys");
    assert_eq!(p["description"], "Repost deploy notices");
    assert_eq!(
        p["trigger"],
        json!({ "kind": "MESSAGE_UPDATED", "conversationId": conversation })
    );
    assert_eq!(
        p["filter"],
        json!({ "conditions": [
            { "conditionType": "MessageContains", "pattern": "deploy",
              "userId": null, "conversationId": null },
            { "conditionType": "MessageFromUser", "pattern": null,
              "userId": author, "conversationId": null },
            { "conditionType": "ConversationIs", "pattern": null,
              "userId": null, "conversationId": conversation },
        ] })
    );
    assert_eq!(
        p["actions"],
        json!([
            { "actionType": "ExecuteCommand", "commandId": "chat.post_message",
              "args": { "text": "seen" } },
            { "actionType": "ExecuteCommand", "commandId": "chat.react", "args": null },
        ])
    );
    assert_eq!(p["enabled"], true);
    assert_eq!(p["cooldownSecs"], 30);
    assert_eq!(p["createdBy"], f.alice.user_id.to_string().as_str());
    assert_eq!(p["schemaVersion"], 1);
    assert_eq!(p["lastFiredAt"], Value::Null);
    assert!(p["createdAt"].is_string());
    assert!(p["updatedAt"].is_string());

    assert_eq!(
        f.list(&f.bob).await,
        [p],
        "the create response is what a read returns"
    );
}

#[tokio::test]
async fn create_pipeline_leaves_the_optional_fields_null() {
    let f = setup().await;
    let p = f.create(&f.alice, minimal_body("bare")).await;
    assert_eq!(p["description"], Value::Null);
    assert_eq!(
        p["trigger"],
        json!({ "kind": "NEW_MESSAGE", "conversationId": null })
    );
    assert_eq!(p["filter"], Value::Null);
    assert_eq!(p["cooldownSecs"], Value::Null);
}

#[tokio::test]
async fn create_pipeline_refuses_an_invalid_name_cooldown_or_trigger() {
    let f = setup().await;
    let conversation = uuid::Uuid::now_v7().to_string();
    let invalid = [
        with(minimal_body(""), json!({})),
        minimal_body(&"n".repeat(257)),
        with(minimal_body("p"), json!({ "cooldownSecs": -1 })),
        with(
            minimal_body("p"),
            json!({ "trigger": { "kind": "MEMBER_JOINED", "conversationId": conversation } }),
        ),
        with(
            minimal_body("p"),
            json!({ "trigger": { "kind": "WORKSPACE_STATE_CHANGED",
                                 "conversationId": conversation } }),
        ),
    ];
    for body in invalid {
        assert_eq!(
            f.create_err(&f.alice, body.clone()).await,
            "VALIDATION",
            "{body}"
        );
    }
    assert!(f.list_names(&f.alice, None).await.is_empty());

    let valid = [
        minimal_body(&"n".repeat(256)),
        with(minimal_body("zero cooldown"), json!({ "cooldownSecs": 0 })),
        with(
            minimal_body("joined"),
            json!({ "trigger": { "kind": "MEMBER_JOINED" } }),
        ),
        with(
            minimal_body("reactions"),
            json!({ "trigger": { "kind": "REACTIONS_UPDATED", "conversationId": conversation } }),
        ),
    ];
    for body in valid {
        f.create(&f.alice, body).await;
    }
}

#[tokio::test]
async fn create_pipeline_refuses_a_filter_condition_missing_its_operand() {
    let f = setup().await;
    let cases = [
        (
            json!({ "conditionType": "MessageContains" }),
            "MessageContains requires a pattern",
        ),
        (
            json!({ "conditionType": "MessageFromUser" }),
            "MessageFromUser requires a user_id",
        ),
        (
            json!({ "conditionType": "ConversationIs" }),
            "ConversationIs requires a conversation_id",
        ),
        (
            json!({ "conditionType": "MessageMatches", "pattern": "x" }),
            "unknown filter condition type: MessageMatches",
        ),
    ];
    for (condition, expected) in cases {
        let body = with(
            minimal_body("p"),
            json!({ "filter": { "conditions": [condition] } }),
        );
        let err = exec_err(&f.schema, f.create_request(&f.alice, &f.server_id, body)).await;
        assert_eq!(err, ("VALIDATION".to_string(), expected.to_string()));
    }
    assert!(f.list_names(&f.alice, None).await.is_empty());
}

#[tokio::test]
async fn a_member_creates_at_most_fifty_pipelines_per_workspace() {
    let f = setup().await;
    for i in 0..50 {
        f.create(&f.alice, minimal_body(&format!("p{i}"))).await;
    }
    assert_eq!(
        f.create_err(&f.alice, minimal_body("p50")).await,
        "VALIDATION"
    );
    f.create(&f.bob, minimal_body("bob's")).await;
}

// ── List ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pipelines_lists_every_members_pipelines_of_the_workspace_only() {
    let f = setup().await;
    f.create(&f.alice, minimal_body("alice's")).await;
    f.create(&f.bob, minimal_body("bob's")).await;
    exec_ok(
        &f.schema,
        f.create_request(&f.olive, &f.other_server_id, minimal_body("elsewhere")),
    )
    .await;

    assert_eq!(f.list_names(&f.gina, None).await, ["alice's", "bob's"]);
}

// ── Toggle ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn toggle_pipeline_switches_it_off_and_on_and_the_enabled_filter_follows() {
    let f = setup().await;
    let created = f.create(&f.alice, minimal_body("toggled")).await;
    let id = id_of(&created);
    f.create(&f.alice, minimal_body("steady")).await;

    let data = exec_ok(&f.schema, f.toggle_request(&f.alice, &id, false)).await;
    let off = &data["pipeline"];
    assert_eq!(off["id"], id.as_str());
    assert_eq!(off["enabled"], false);
    assert_eq!(off["name"], "toggled");
    assert_eq!(off["createdAt"], created["createdAt"]);
    assert_ne!(off["updatedAt"], created["updatedAt"]);

    assert_eq!(f.list_names(&f.alice, Some(false)).await, ["toggled"]);
    assert_eq!(f.list_names(&f.alice, Some(true)).await, ["steady"]);
    assert_eq!(f.list_names(&f.alice, None).await, ["steady", "toggled"]);

    let data = exec_ok(&f.schema, f.toggle_request(&f.alice, &id, true)).await;
    assert_eq!(data["pipeline"]["enabled"], true);
    assert!(f.list_names(&f.alice, Some(false)).await.is_empty());
}

#[tokio::test]
async fn toggle_pipeline_refuses_an_unknown_or_malformed_id() {
    let f = setup().await;
    let unknown = uuid::Uuid::now_v7().to_string();
    assert_eq!(
        exec_err_code(&f.schema, f.toggle_request(&f.alice, &unknown, false)).await,
        "NOT_FOUND"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.toggle_request(&f.alice, "not-a-uuid", false)).await,
        "VALIDATION"
    );
}

// ── Update ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_pipeline_replaces_the_definition_and_keeps_state_and_authorship() {
    let f = setup().await;
    let created = f
        .create(
            &f.alice,
            with(
                minimal_body("before"),
                json!({ "description": "old",
                        "filter": { "conditions": [
                            { "conditionType": "MessageContains", "pattern": "x" }] },
                        "cooldownSecs": 10 }),
            ),
        )
        .await;
    let id = id_of(&created);
    exec_ok(&f.schema, f.toggle_request(&f.alice, &id, false)).await;

    let body = json!({
        "name": "after",
        "trigger": { "kind": "MESSAGE_DELETED" },
        "actions": [{ "commandId": "chat.react", "args": { "emoji": "x" } }],
    });
    let data = exec_ok(&f.schema, f.update_request(&f.alice, &id, body)).await;
    let updated = &data["pipeline"];
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["name"], "after");
    assert_eq!(
        updated["description"],
        Value::Null,
        "omitted fields are cleared"
    );
    assert_eq!(updated["filter"], Value::Null);
    assert_eq!(updated["cooldownSecs"], Value::Null);
    assert_eq!(
        updated["trigger"],
        json!({ "kind": "MESSAGE_DELETED", "conversationId": null })
    );
    assert_eq!(
        updated["actions"],
        json!([{ "actionType": "ExecuteCommand", "commandId": "chat.react",
                 "args": { "emoji": "x" } }])
    );
    assert_eq!(
        updated["enabled"], false,
        "an update keeps the pipeline switched off"
    );
    assert_eq!(updated["createdBy"], created["createdBy"]);
    assert_eq!(updated["schemaVersion"], 1);
    assert_eq!(updated["createdAt"], created["createdAt"]);

    let listed = f.list(&f.alice).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "after");
    assert_eq!(listed[0]["trigger"], updated["trigger"]);
    assert_eq!(listed[0]["enabled"], false);
}

#[tokio::test]
async fn update_pipeline_validates_like_create() {
    let f = setup().await;
    let id = id_of(&f.create(&f.alice, minimal_body("p")).await);
    let conversation = uuid::Uuid::now_v7().to_string();
    let invalid = [
        minimal_body(""),
        minimal_body(&"n".repeat(257)),
        with(minimal_body("p"), json!({ "cooldownSecs": -1 })),
        with(
            minimal_body("p"),
            json!({ "trigger": { "kind": "MEMBER_JOINED", "conversationId": conversation } }),
        ),
    ];
    for body in invalid {
        assert_eq!(
            exec_err_code(&f.schema, f.update_request(&f.alice, &id, body.clone())).await,
            "VALIDATION",
            "{body}"
        );
    }
    let err = exec_err(
        &f.schema,
        f.update_request(
            &f.alice,
            &id,
            with(
                minimal_body("p"),
                json!({ "filter": { "conditions": [{ "conditionType": "MessageContains" }] } }),
            ),
        ),
    )
    .await;
    assert_eq!(
        err,
        (
            "VALIDATION".to_string(),
            "MessageContains requires a pattern".to_string()
        )
    );
    assert_eq!(f.list_names(&f.alice, None).await, ["p"]);
}

#[tokio::test]
async fn update_pipeline_gets_not_found_for_an_unknown_pipeline() {
    let f = setup().await;
    let unknown = uuid::Uuid::now_v7().to_string();
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.update_request(&f.alice, &unknown, minimal_body("p"))
        )
        .await,
        "NOT_FOUND"
    );
}

// ── Delete ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_pipeline_removes_it_and_a_second_delete_is_not_found() {
    let f = setup().await;
    let id = id_of(&f.create(&f.alice, minimal_body("doomed")).await);
    f.create(&f.alice, minimal_body("kept")).await;

    let data = exec_ok(&f.schema, f.delete_request(&f.alice, &id)).await;
    assert_eq!(data["deleted"], true);
    assert_eq!(f.list_names(&f.alice, None).await, ["kept"]);

    for request in [
        f.delete_request(&f.alice, &id),
        f.toggle_request(&f.alice, &id, true),
        f.update_request(&f.alice, &id, minimal_body("back")),
    ] {
        assert_eq!(exec_err_code(&f.schema, request).await, "NOT_FOUND");
    }
}
