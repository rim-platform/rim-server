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
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Call, Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err,
    exec_err_code, exec_ok,
};
use support::roles::Roles;
use support::{TestCtx, unknown_id};

macro_rules! pipeline_fields {
    () => {
        "id serverId name description trigger { kind conversationId } \
         filter { conditions { conditionType pattern userId conversationId } } \
         actions { actionType commandId args } \
         enabled cooldownSecs createdBy schemaVersion lastFiredAt createdAt updatedAt"
    };
}

const CREATE_PIPELINE: &str = concat!(
    "mutation Create($input: CreatePipelineInput!) { pipeline: createPipeline(input: $input) { ",
    pipeline_fields!(),
    " } }"
);

const UPDATE_PIPELINE: &str = concat!(
    "mutation Update($input: UpdatePipelineInput!) { pipeline: updatePipeline(input: $input) { ",
    pipeline_fields!(),
    " } }"
);

const TOGGLE_PIPELINE: &str = concat!(
    "mutation Toggle($pipelineId: ID!, $enabled: Boolean!) { ",
    "pipeline: togglePipeline(pipelineId: $pipelineId, enabled: $enabled) { ",
    pipeline_fields!(),
    " } }"
);

const LIST_PIPELINES: &str = concat!(
    "query List($serverId: ID!, $enabled: Boolean) { ",
    "pipelines: pipelines(serverId: $serverId, enabled: $enabled) { ",
    pipeline_fields!(),
    " } }"
);

const DELETE_PIPELINE: &str = r#"
mutation Delete($pipelineId: ID!) { deleted: deletePipeline(pipelineId: $pipelineId) }
"#;

struct Fixture {
    schema: AppSchema,
    ws: Roles,
    _ctx: TestCtx,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    Fixture {
        schema: ctx.build_schema(RegistrationMode::InviteOnly).await,
        ws: ctx.roles("pipe").await,
        _ctx: ctx,
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

/// `minimal_body` with a filter of the one condition `condition`.
fn with_condition(condition: Value) -> Value {
    with(
        minimal_body("p"),
        json!({ "filter": { "conditions": [condition] } }),
    )
}

fn id_of(pipeline: &Value) -> String {
    pipeline["id"].as_str().expect("pipeline id").to_string()
}

impl Fixture {
    /// Create `body` in `server_id`.
    fn create_in(&self, server_id: &str, body: Value) -> Call {
        let input = with(body, json!({ "serverId": server_id }));
        (CREATE_PIPELINE, json!({ "input": input }))
    }

    fn create_call(&self, body: Value) -> Call {
        self.create_in(&self.ws.server_id, body)
    }

    fn update_call(&self, pipeline_id: &str, body: Value) -> Call {
        let input = with(body, json!({ "pipelineId": pipeline_id }));
        (UPDATE_PIPELINE, json!({ "input": input }))
    }

    fn toggle_call(&self, pipeline_id: &str, enabled: bool) -> Call {
        (
            TOGGLE_PIPELINE,
            json!({ "pipelineId": pipeline_id, "enabled": enabled }),
        )
    }

    fn delete_call(&self, pipeline_id: &str) -> Call {
        (DELETE_PIPELINE, json!({ "pipelineId": pipeline_id }))
    }

    fn list_call(&self, enabled: Option<bool>) -> Call {
        (
            LIST_PIPELINES,
            json!({ "serverId": self.ws.server_id, "enabled": enabled }),
        )
    }

    /// One call to every pipeline root that changes pipeline `id`.
    fn every_change(&self, id: &str) -> Vec<Call> {
        vec![
            self.update_call(id, minimal_body("taken")),
            self.toggle_call(id, false),
            self.delete_call(id),
        ]
    }

    /// One call to every pipeline root, the changes aimed at pipeline `id`.
    fn every_root(&self, id: &str) -> Vec<Call> {
        let mut calls = vec![self.list_call(None), self.create_call(minimal_body("p"))];
        calls.extend(self.every_change(id));
        calls
    }

    async fn ok(&self, who: &Caller, call: Call) -> Value {
        exec_ok(&self.schema, who.call(call)).await
    }

    async fn code(&self, who: &Caller, call: Call) -> String {
        exec_err_code(&self.schema, who.call(call)).await
    }

    async fn create(&self, who: &Caller, body: Value) -> Value {
        self.ok(who, self.create_call(body)).await["pipeline"].clone()
    }

    /// Create a minimal pipeline named `name` as alice and return its id.
    async fn pipeline(&self, name: &str) -> String {
        id_of(&self.create(&self.ws.alice, minimal_body(name)).await)
    }

    async fn list(&self, who: &Caller) -> Vec<Value> {
        let data = self.ok(who, self.list_call(None)).await;
        data["pipelines"].as_array().expect("pipelines").clone()
    }

    async fn list_names(&self, who: &Caller, enabled: Option<bool>) -> Vec<String> {
        let data = self.ok(who, self.list_call(enabled)).await;
        let mut names: Vec<String> = data["pipelines"]
            .as_array()
            .expect("pipelines")
            .iter()
            .map(|p| p["name"].as_str().expect("name").to_string())
            .collect();
        names.sort();
        names
    }
}

// ── Authentication and membership ─────────────────────────────────────────

#[tokio::test]
async fn every_pipeline_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    let id = f.pipeline("p").await;
    assert_each_refused(
        &f.schema,
        &f.every_root(&id),
        "UNAUTHENTICATED",
        anonymous_request,
    )
    .await;
}

#[tokio::test]
async fn every_pipeline_root_refuses_a_non_member() {
    let f = setup().await;
    let id = f.pipeline("p").await;
    assert_each_refused_as(&f.schema, &f.ws.olive, &f.every_root(&id), "FORBIDDEN").await;
    let listed = f.list(&f.ws.alice).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "p");
    assert_eq!(listed[0]["enabled"], true);
}

#[tokio::test]
async fn a_guest_may_create_a_pipeline() {
    let f = setup().await;
    let created = f.create(&f.ws.gina, minimal_body("guest pipeline")).await;
    assert_eq!(created["createdBy"], f.ws.gina.user_id.to_string().as_str());
    assert_eq!(f.list_names(&f.ws.alice, None).await, ["guest pipeline"]);
}

#[tokio::test]
async fn only_its_creator_or_a_moderator_may_change_a_pipeline() {
    let f = setup().await;
    let id = f.pipeline("owner's").await;
    assert_each_refused_as(&f.schema, &f.ws.bob, &f.every_change(&id), "FORBIDDEN").await;
    assert_eq!(f.list_names(&f.ws.alice, Some(true)).await, ["owner's"]);

    let mona = &f.ws.mona;
    let data = f
        .ok(mona, f.update_call(&id, minimal_body("moderated")))
        .await;
    assert_eq!(
        data["pipeline"]["createdBy"],
        f.ws.alice.user_id.to_string().as_str()
    );
    let data = f.ok(mona, f.toggle_call(&id, false)).await;
    assert_eq!(data["pipeline"]["enabled"], false);
    let data = f.ok(mona, f.delete_call(&id)).await;
    assert_eq!(data["deleted"], true);
}

// ── Create ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_pipeline_returns_an_enabled_pipeline_with_every_field() {
    let f = setup().await;
    let conversation = unknown_id();
    let author = f.ws.bob.user_id.to_string();
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
            { "commandId": "chat.toggle_reaction" },
        ],
        "cooldownSecs": 30,
    });
    let p = f.create(&f.ws.alice, body).await;

    assert!(p["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(p["serverId"], f.ws.server_id.as_str());
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
            { "actionType": "ExecuteCommand", "commandId": "chat.toggle_reaction", "args": null },
        ])
    );
    assert_eq!(p["enabled"], true);
    assert_eq!(p["cooldownSecs"], 30);
    assert_eq!(p["createdBy"], f.ws.alice.user_id.to_string().as_str());
    assert_eq!(p["schemaVersion"], 1);
    assert_eq!(p["lastFiredAt"], Value::Null);
    assert!(p["createdAt"].is_string());
    assert!(p["updatedAt"].is_string());

    assert_eq!(
        f.list(&f.ws.bob).await,
        [p],
        "the create response is what a read returns"
    );
}

#[tokio::test]
async fn create_pipeline_leaves_the_optional_fields_null() {
    let f = setup().await;
    let p = f.create(&f.ws.alice, minimal_body("bare")).await;
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
    let conversation = unknown_id();
    let invalid = [
        minimal_body(""),
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
    assert_each_refused_as(
        &f.schema,
        &f.ws.alice,
        &invalid.map(|b| f.create_call(b)),
        "VALIDATION",
    )
    .await;
    assert!(f.list_names(&f.ws.alice, None).await.is_empty());

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
        f.create(&f.ws.alice, body).await;
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
        let call = f.create_call(with_condition(condition));
        let err = exec_err(&f.schema, f.ws.alice.call(call)).await;
        assert_eq!(err, ("VALIDATION".to_string(), expected.to_string()));
    }
    assert!(f.list_names(&f.ws.alice, None).await.is_empty());
}

/// A pipeline holds at most ten actions of 4 KiB of arguments each, none naming a user command
/// or a macro, five filter conditions with patterns of 256 characters, and a description of
/// 1,000 characters; `createPipeline` and `updatePipeline` refuse more with `VALIDATION`.
#[tokio::test]
async fn create_and_update_pipeline_refuse_definitions_past_their_limits() {
    let f = setup().await;
    let search = json!({ "commandId": "chat.search_messages" });
    let contains =
        |pattern: String| json!({ "conditionType": "MessageContains", "pattern": pattern });
    let too_big = [
        with(
            minimal_body("p"),
            json!({ "actions": vec![search.clone(); 11] }),
        ),
        with(
            minimal_body("p"),
            json!({ "actions": [{ "commandId": "chat.search_messages",
                                  "args": { "query": "x".repeat(4096) } }] }),
        ),
        with(
            minimal_body("p"),
            json!({ "actions": [{ "commandId": "user.many_steps" }] }),
        ),
        with(
            minimal_body("p"),
            json!({ "description": "d".repeat(1001) }),
        ),
        with(
            minimal_body("p"),
            json!({ "filter": { "conditions": vec![contains("bye".to_string()); 6] } }),
        ),
        with_condition(contains("p".repeat(257))),
    ];
    let stored = f.pipeline("stored").await;
    for body in too_big {
        for call in [f.create_call(body.clone()), f.update_call(&stored, body)] {
            assert_eq!(f.code(&f.ws.alice, call).await, "VALIDATION");
        }
    }

    let at_the_limits = with(
        minimal_body("at the limits"),
        json!({
            "description": "d".repeat(1000),
            "actions": vec![search; 10],
            "filter": { "conditions": vec![contains("p".repeat(256)); 5] },
        }),
    );
    f.create(&f.ws.alice, at_the_limits).await;
}

#[tokio::test]
async fn a_member_creates_at_most_fifty_pipelines_per_workspace() {
    let f = setup().await;
    for i in 0..50 {
        f.create(&f.ws.alice, minimal_body(&format!("p{i}"))).await;
    }
    assert_eq!(
        f.code(&f.ws.alice, f.create_call(minimal_body("p50")))
            .await,
        "VALIDATION"
    );
    f.create(&f.ws.bob, minimal_body("bob's")).await;
}

// ── List ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pipelines_lists_every_members_pipelines_of_the_workspace_only() {
    let f = setup().await;
    f.create(&f.ws.alice, minimal_body("alice's")).await;
    f.create(&f.ws.bob, minimal_body("bob's")).await;
    let elsewhere = f.create_in(&f.ws.other_server_id, minimal_body("elsewhere"));
    f.ok(&f.ws.olive, elsewhere).await;

    assert_eq!(f.list_names(&f.ws.gina, None).await, ["alice's", "bob's"]);
}

// ── Toggle ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn toggle_pipeline_switches_it_off_and_on_and_the_enabled_filter_follows() {
    let f = setup().await;
    let created = f.create(&f.ws.alice, minimal_body("toggled")).await;
    let id = id_of(&created);
    f.pipeline("steady").await;

    let data = f.ok(&f.ws.alice, f.toggle_call(&id, false)).await;
    let off = &data["pipeline"];
    assert_eq!(off["id"], id.as_str());
    assert_eq!(off["enabled"], false);
    assert_eq!(off["name"], "toggled");
    assert_eq!(off["createdAt"], created["createdAt"]);
    assert_ne!(off["updatedAt"], created["updatedAt"]);

    assert_eq!(f.list_names(&f.ws.alice, Some(false)).await, ["toggled"]);
    assert_eq!(f.list_names(&f.ws.alice, Some(true)).await, ["steady"]);
    assert_eq!(f.list_names(&f.ws.alice, None).await, ["steady", "toggled"]);

    let data = f.ok(&f.ws.alice, f.toggle_call(&id, true)).await;
    assert_eq!(data["pipeline"]["enabled"], true);
    assert!(f.list_names(&f.ws.alice, Some(false)).await.is_empty());
}

#[tokio::test]
async fn toggle_pipeline_refuses_an_unknown_or_malformed_id() {
    let f = setup().await;
    assert_eq!(
        f.code(&f.ws.alice, f.toggle_call(&unknown_id(), false))
            .await,
        "NOT_FOUND"
    );
    assert_eq!(
        f.code(&f.ws.alice, f.toggle_call("not-a-uuid", false))
            .await,
        "VALIDATION"
    );
}

// ── Update ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_pipeline_replaces_the_definition_and_keeps_state_and_authorship() {
    let f = setup().await;
    let created = f
        .create(
            &f.ws.alice,
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
    f.ok(&f.ws.alice, f.toggle_call(&id, false)).await;

    let body = json!({
        "name": "after",
        "trigger": { "kind": "MESSAGE_DELETED" },
        "actions": [{ "commandId": "chat.toggle_reaction", "args": { "emoji": "x" } }],
    });
    let data = f.ok(&f.ws.alice, f.update_call(&id, body)).await;
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
        json!([{ "actionType": "ExecuteCommand", "commandId": "chat.toggle_reaction",
                 "args": { "emoji": "x" } }])
    );
    assert_eq!(
        updated["enabled"], false,
        "an update keeps the pipeline switched off"
    );
    assert_eq!(updated["createdBy"], created["createdBy"]);
    assert_eq!(updated["schemaVersion"], 1);
    assert_eq!(updated["createdAt"], created["createdAt"]);

    let listed = f.list(&f.ws.alice).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "after");
    assert_eq!(listed[0]["trigger"], updated["trigger"]);
    assert_eq!(listed[0]["enabled"], false);
}

#[tokio::test]
async fn update_pipeline_validates_like_create() {
    let f = setup().await;
    let id = f.pipeline("p").await;
    let conversation = unknown_id();
    let invalid = [
        minimal_body(""),
        minimal_body(&"n".repeat(257)),
        with(minimal_body("p"), json!({ "cooldownSecs": -1 })),
        with(
            minimal_body("p"),
            json!({ "trigger": { "kind": "MEMBER_JOINED", "conversationId": conversation } }),
        ),
    ];
    assert_each_refused_as(
        &f.schema,
        &f.ws.alice,
        &invalid.map(|b| f.update_call(&id, b)),
        "VALIDATION",
    )
    .await;
    let call = f.update_call(
        &id,
        with_condition(json!({ "conditionType": "MessageContains" })),
    );
    let err = exec_err(&f.schema, f.ws.alice.call(call)).await;
    assert_eq!(
        err,
        (
            "VALIDATION".to_string(),
            "MessageContains requires a pattern".to_string()
        )
    );
    assert_eq!(f.list_names(&f.ws.alice, None).await, ["p"]);
}

/// `createPipeline` and `updatePipeline` refuse an action naming no builtin command, a plugin
/// command included, with `VALIDATION` naming the id; the stored actions stay as they were.
#[tokio::test]
async fn create_and_update_pipeline_refuse_an_unknown_command() {
    let f = setup().await;
    let unknown = |command_id: &str| {
        with(
            minimal_body("p"),
            json!({ "actions": [{ "commandId": command_id }] }),
        )
    };
    let err = exec_err(
        &f.schema,
        f.ws.alice.call(f.create_call(unknown("chat.react"))),
    )
    .await;
    assert_eq!(
        err,
        (
            "VALIDATION".to_string(),
            "Unknown command: chat.react".to_string()
        )
    );

    let created = f.create(&f.ws.alice, minimal_body("p")).await;
    let call = f.update_call(&id_of(&created), unknown("plugin.dev.example.x"));
    let err = exec_err(&f.schema, f.ws.alice.call(call)).await;
    assert_eq!(
        err,
        (
            "VALIDATION".to_string(),
            "Unknown command: plugin.dev.example.x".to_string()
        )
    );
    let listed = f.list(&f.ws.alice).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["actions"], created["actions"]);
}

#[tokio::test]
async fn update_pipeline_gets_not_found_for_an_unknown_pipeline() {
    let f = setup().await;
    assert_eq!(
        f.code(&f.ws.alice, f.update_call(&unknown_id(), minimal_body("p")))
            .await,
        "NOT_FOUND"
    );
}

// ── Delete ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_pipeline_removes_it_and_a_second_delete_is_not_found() {
    let f = setup().await;
    let id = f.pipeline("doomed").await;
    f.pipeline("kept").await;

    let data = f.ok(&f.ws.alice, f.delete_call(&id)).await;
    assert_eq!(data["deleted"], true);
    assert_eq!(f.list_names(&f.ws.alice, None).await, ["kept"]);

    assert_each_refused_as(&f.schema, &f.ws.alice, &f.every_change(&id), "NOT_FOUND").await;
}
