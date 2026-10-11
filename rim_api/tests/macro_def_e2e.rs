//! Through-the-stack e2e tests for the macro GraphQL contract: `createMacro`, `updateMacro`,
//! `deleteMacro`, `executeMacro`, `myMacros` and `macroByName`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_feature::model::ServerFeature;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};

const MACRO_FIELDS: &str = r#"
fragment MacroFields on MacroDef {
  id serverId userId name description steps { commandId args } aiCallable nlDescription
  intentSamples sensitivity createdAt
}
"#;

const CREATE_MACRO: &str = r#"
mutation Create($input: CreateMacroInput!) { macro: createMacro(input: $input) { ...MacroFields } }
"#;

const UPDATE_MACRO: &str = r#"
mutation Update($input: UpdateMacroInput!) { macro: updateMacro(input: $input) { ...MacroFields } }
"#;

const DELETE_MACRO: &str = r#"
mutation Delete($macroId: ID!) { deleteMacro(macroId: $macroId) }
"#;

const EXECUTE_MACRO: &str = r#"
mutation Execute($input: ExecuteMacroInput!) {
  result: executeMacro(input: $input) {
    status message payload followUpCommands confirmationRequired { token argsHash expiresAt }
  }
}
"#;

const MY_MACROS: &str = r#"
query Mine($serverId: ID!) { macros: myMacros(serverId: $serverId) { ...MacroFields } }
"#;

const MACRO_BY_NAME: &str = r#"
query ByName($serverId: ID!, $name: String!) {
  macro: macroByName(serverId: $serverId, name: $name) { ...MacroFields }
}
"#;

const CREATE_CHANNEL: &str = r#"
mutation CreateChannel($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

const MESSAGES_QUERY: &str = r#"
query Messages($serverId: ID!, $conversationId: ID!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: AFTER, limit: 50,
                 includeDeleted: false) { items { text authorId } }
}
"#;

fn with_fields(query: &str) -> String {
    format!("{query}{MACRO_FIELDS}")
}

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server_id: String,
    /// Owner; created the public channel.
    alice: Caller,
    bob: Caller,
    /// Owner of another workspace, not a member of this one.
    olaf: Caller,
    channel_id: String,
}

async fn caller(ctx: &TestCtx, user: &User) -> Caller {
    Caller {
        user_id: user.id,
        session_id: ctx.create_session(user).await,
    }
}

/// alice (Owner) and bob (Member) in one workspace with a public channel alice created; olaf
/// owns another workspace.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    ctx.seed_capabilities().await;
    let server = ctx.create_server("macro-e2e").await;
    let other = ctx.create_server("macro-e2e-other").await;
    let alice_user = ctx.create_user("macro-alice").await;
    let bob_user = ctx.create_user("macro-bob").await;
    let olaf_user = ctx.create_user("macro-olaf").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_owner(&other, &olaf_user).await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
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
        olaf: caller(&ctx, &olaf_user).await,
        alice,
        server_id,
        channel_id,
        schema,
        ctx,
    }
}

impl Fixture {
    /// A step posting `text` into the fixture channel.
    fn post_step(&self, text: &str) -> Value {
        json!({
            "commandId": "chat.post_message",
            "args": {
                "server_id": self.server_id,
                "conversation_id": self.channel_id,
                "text": text,
            },
        })
    }

    /// A read-only step listing the workspace's channels.
    fn list_step(&self) -> Value {
        json!({ "commandId": "chat.list_channels", "args": { "server_id": self.server_id } })
    }

    /// Create an AI-callable macro and return its id.
    async fn create_ai_callable(&self, who: &Caller, name: &str, steps: Value) -> String {
        let data = exec_ok(
            &self.schema,
            who.request(
                &with_fields(CREATE_MACRO),
                json!({ "input": {
                    "serverId": self.server_id, "name": name, "steps": steps, "aiCallable": true,
                } }),
            ),
        )
        .await;
        data["macro"]["id"].as_str().expect("id").to_string()
    }

    fn create_input(&self, name: &str, steps: Value) -> Value {
        json!({ "input": { "serverId": self.server_id, "name": name, "steps": steps } })
    }

    async fn create(&self, who: &Caller, name: &str, steps: Value) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(&with_fields(CREATE_MACRO), self.create_input(name, steps)),
        )
        .await;
        data["macro"].clone()
    }

    async fn create_code(&self, who: &Caller, name: &str, steps: Value) -> String {
        exec_err_code(
            &self.schema,
            who.request(&with_fields(CREATE_MACRO), self.create_input(name, steps)),
        )
        .await
    }

    async fn mine(&self, who: &Caller) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(
                &with_fields(MY_MACROS),
                json!({ "serverId": self.server_id }),
            ),
        )
        .await;
        data["macros"].as_array().expect("macros").clone()
    }

    async fn by_name(&self, who: &Caller, name: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(
                &with_fields(MACRO_BY_NAME),
                json!({ "serverId": self.server_id, "name": name }),
            ),
        )
        .await;
        data["macro"].clone()
    }

    fn update_vars(&self, macro_id: &str, name: &str, steps: Value) -> Value {
        json!({ "input": { "macroId": macro_id, "name": name, "steps": steps } })
    }

    fn execute_vars(&self, macro_id: &str, extra: Value) -> Value {
        let mut input = json!({
            "macroId": macro_id,
            "commandContext": { "workspaceId": self.server_id },
        });
        if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
            input.extend(extra.clone());
        }
        json!({ "input": input })
    }

    async fn channel_texts(&self) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            self.alice.request(
                MESSAGES_QUERY,
                json!({ "serverId": self.server_id, "conversationId": self.channel_id }),
            ),
        )
        .await;
        data["page"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|m| m["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// Every macro root answers `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn macro_roots_refuse_anonymous_callers() {
    let f = setup().await;
    let id = uuid::Uuid::now_v7().to_string();
    let calls = [
        (
            with_fields(CREATE_MACRO),
            f.create_input("m", json!([f.post_step("x")])),
        ),
        (
            with_fields(UPDATE_MACRO),
            f.update_vars(&id, "m", json!([f.post_step("x")])),
        ),
        (DELETE_MACRO.to_string(), json!({ "macroId": id })),
        (EXECUTE_MACRO.to_string(), f.execute_vars(&id, json!({}))),
        (with_fields(MY_MACROS), json!({ "serverId": f.server_id })),
        (
            with_fields(MACRO_BY_NAME),
            json!({ "serverId": f.server_id, "name": "m" }),
        ),
    ];
    for (query, vars) in calls {
        let code = exec_err_code(&f.schema, anonymous_request(&query, vars)).await;
        assert_eq!(code, "UNAUTHENTICATED", "{query}");
    }
}

/// A user outside the workspace cannot create a macro in it or look one up by name there, and
/// lists none of its macros.
#[tokio::test]
async fn macro_roots_refuse_non_members() {
    let f = setup().await;
    f.create(&f.alice, "greet", json!([f.post_step("hi")]))
        .await;

    let code = f
        .create_code(&f.olaf, "intrude", json!([f.post_step("x")]))
        .await;
    assert_eq!(code, "FORBIDDEN");
    let code = exec_err_code(
        &f.schema,
        f.olaf.request(
            &with_fields(MACRO_BY_NAME),
            json!({ "serverId": f.server_id, "name": "greet" }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");
    assert!(f.mine(&f.olaf).await.is_empty());
}

/// `createMacro` returns every field of the new macro, with the defaults for omitted inputs,
/// and the macro is then listed and found by name for its author only.
#[tokio::test]
async fn create_macro_returns_the_macro_and_lists_it_for_its_author() {
    let f = setup().await;
    let before = chrono::Utc::now();
    let created = exec_ok(
        &f.schema,
        f.bob.request(
            &with_fields(CREATE_MACRO),
            json!({ "input": {
                "serverId": f.server_id,
                "name": "  standup  ",
                "description": "post the standup note",
                "steps": [f.post_step("standup"), { "commandId": "chat.mark_read" }],
                "aiCallable": true,
                "nlDescription": "posts the standup note",
                "intentSamples": ["standup please"],
                "sensitivity": "MUTATING",
            } }),
        ),
    )
    .await["macro"]
        .clone();

    assert_eq!(created["serverId"], json!(f.server_id));
    assert_eq!(created["userId"], json!(f.bob.user_id.to_string()));
    assert_eq!(created["name"], json!("standup"), "the name is trimmed");
    assert_eq!(created["description"], json!("post the standup note"));
    assert_eq!(
        created["steps"],
        json!([
            {
                "commandId": "chat.post_message",
                "args": { "server_id": f.server_id, "conversation_id": f.channel_id, "text": "standup" },
            },
            { "commandId": "chat.mark_read", "args": null },
        ])
    );
    assert_eq!(created["aiCallable"], json!(true));
    assert_eq!(created["nlDescription"], json!("posts the standup note"));
    assert_eq!(created["intentSamples"], json!(["standup please"]));
    assert_eq!(created["sensitivity"], json!("MUTATING"));
    let created_at: chrono::DateTime<chrono::Utc> = created["createdAt"]
        .as_str()
        .expect("createdAt")
        .parse()
        .expect("RFC 3339");
    assert!(created_at >= before - chrono::Duration::seconds(1));

    let defaults = f.create(&f.bob, "plain", json!([f.post_step("x")])).await;
    assert_eq!(defaults["description"], Value::Null);
    assert_eq!(defaults["aiCallable"], json!(false));
    assert_eq!(defaults["nlDescription"], json!(""));
    assert_eq!(defaults["intentSamples"], json!([]));
    assert_eq!(defaults["sensitivity"], json!("SAFE"));

    let mine = f.mine(&f.bob).await;
    let mut names: Vec<_> = mine.iter().map(|m| m["name"].clone()).collect();
    names.sort_by_key(|n| n.to_string());
    assert_eq!(names, vec![json!("plain"), json!("standup")]);
    assert_eq!(f.by_name(&f.bob, "standup").await, created);

    assert!(f.mine(&f.alice).await.is_empty(), "macros are per user");
    assert_eq!(f.by_name(&f.alice, "standup").await, Value::Null);
    assert_eq!(f.by_name(&f.bob, "missing").await, Value::Null);
}

/// `createMacro` refuses a blank or over-long name, no steps, more than twenty steps and a step
/// that calls another macro, with `VALIDATION`.
#[tokio::test]
async fn create_macro_validates_name_and_steps() {
    let f = setup().await;
    let one = json!([f.post_step("x")]);
    let cases = [
        ("   ".to_string(), one.clone()),
        ("n".repeat(129), one.clone()),
        ("empty".to_string(), json!([])),
        (
            "long".to_string(),
            Value::Array((0..21).map(|i| f.post_step(&i.to_string())).collect()),
        ),
        (
            "nested".to_string(),
            json!([{ "commandId": "macro.anything" }]),
        ),
    ];
    for (name, steps) in cases {
        let code = f.create_code(&f.bob, &name, steps).await;
        assert_eq!(code, "VALIDATION", "{name}");
    }

    let twenty = Value::Array((0..20).map(|i| f.post_step(&i.to_string())).collect());
    f.create(&f.bob, &"n".repeat(128), twenty).await;
    assert_eq!(f.mine(&f.bob).await.len(), 1);
}

/// A second macro with the same name for the same user in the same workspace answers
/// `CONFLICT`; another user may reuse the name.
#[tokio::test]
async fn create_macro_refuses_a_duplicate_name() {
    let f = setup().await;
    f.create(&f.bob, "dup", json!([f.post_step("x")])).await;
    let code = f
        .create_code(&f.bob, "dup", json!([f.post_step("y")]))
        .await;
    assert_eq!(code, "CONFLICT");
    f.create(&f.alice, "dup", json!([f.post_step("x")])).await;
}

/// A user may keep at most fifty macros per workspace; the fifty-first answers `VALIDATION`.
#[tokio::test]
async fn create_macro_caps_macros_per_user() {
    let f = setup().await;
    for i in 0..50 {
        f.create(&f.bob, &format!("m{i}"), json!([f.post_step("x")]))
            .await;
    }
    let code = f
        .create_code(&f.bob, "one_too_many", json!([f.post_step("x")]))
        .await;
    assert_eq!(code, "VALIDATION");
    f.create(&f.alice, "unaffected", json!([f.post_step("x")]))
        .await;
}

/// `macroByName` refuses a name over 128 characters with `VALIDATION`.
#[tokio::test]
async fn macro_by_name_refuses_an_over_long_name() {
    let f = setup().await;
    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            &with_fields(MACRO_BY_NAME),
            json!({ "serverId": f.server_id, "name": "n".repeat(129) }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
}

/// `updateMacro` replaces the author's macro and returns it; anyone else gets `FORBIDDEN`, an
/// unknown id `NOT_FOUND`, and the same name and step rules as creation apply.
#[tokio::test]
async fn update_macro_is_author_only_and_validated() {
    let f = setup().await;
    let created = f.create(&f.bob, "old", json!([f.post_step("x")])).await;
    let id = created["id"].as_str().expect("id").to_string();

    let updated = exec_ok(
        &f.schema,
        f.bob.request(
            &with_fields(UPDATE_MACRO),
            json!({ "input": {
                "macroId": id,
                "name": "new",
                "description": "changed",
                "steps": [f.post_step("y"), f.post_step("z")],
                "aiCallable": true,
                "sensitivity": "RESTRICTED",
            } }),
        ),
    )
    .await["macro"]
        .clone();
    assert_eq!(updated["id"], json!(id));
    assert_eq!(updated["name"], json!("new"));
    assert_eq!(updated["description"], json!("changed"));
    assert_eq!(updated["steps"].as_array().map(Vec::len), Some(2));
    assert_eq!(updated["aiCallable"], json!(true));
    assert_eq!(updated["sensitivity"], json!("RESTRICTED"));
    assert_eq!(updated["createdAt"], created["createdAt"]);
    assert_eq!(f.by_name(&f.bob, "old").await, Value::Null);
    assert_eq!(f.by_name(&f.bob, "new").await, updated);

    for who in [&f.alice, &f.olaf] {
        let code = exec_err_code(
            &f.schema,
            who.request(
                &with_fields(UPDATE_MACRO),
                f.update_vars(&id, "stolen", json!([f.post_step("x")])),
            ),
        )
        .await;
        assert_eq!(code, "FORBIDDEN");
    }

    let missing = uuid::Uuid::now_v7().to_string();
    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            &with_fields(UPDATE_MACRO),
            f.update_vars(&missing, "x", json!([f.post_step("x")])),
        ),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");

    for (name, steps) in [
        (" ", json!([f.post_step("x")])),
        ("new", json!([])),
        ("new", json!([{ "commandId": "macro.other" }])),
    ] {
        let code = exec_err_code(
            &f.schema,
            f.bob
                .request(&with_fields(UPDATE_MACRO), f.update_vars(&id, name, steps)),
        )
        .await;
        assert_eq!(code, "VALIDATION");
    }
    assert_eq!(f.by_name(&f.bob, "new").await, updated);
}

/// `deleteMacro` removes the author's macro and answers `true`; anyone else gets `FORBIDDEN`
/// and a deleted or unknown id `NOT_FOUND`.
#[tokio::test]
async fn delete_macro_is_author_only() {
    let f = setup().await;
    let created = f.create(&f.bob, "gone", json!([f.post_step("x")])).await;
    let id = created["id"].as_str().expect("id").to_string();

    for who in [&f.alice, &f.olaf] {
        let code = exec_err_code(
            &f.schema,
            who.request(DELETE_MACRO, json!({ "macroId": id })),
        )
        .await;
        assert_eq!(code, "FORBIDDEN");
    }
    assert_eq!(f.mine(&f.bob).await.len(), 1);

    let data = exec_ok(
        &f.schema,
        f.bob.request(DELETE_MACRO, json!({ "macroId": id })),
    )
    .await;
    assert_eq!(data["deleteMacro"], json!(true));
    assert!(f.mine(&f.bob).await.is_empty());

    let code = exec_err_code(
        &f.schema,
        f.bob.request(DELETE_MACRO, json!({ "macroId": id })),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");
}

/// `executeMacro` runs every step as the caller and answers `SUCCESS` with each step's payload
/// keyed by its command id.
#[tokio::test]
async fn execute_macro_runs_every_step() {
    let f = setup().await;
    let created = f
        .create(
            &f.bob,
            "twice",
            json!([f.post_step("first"), f.post_step("second")]),
        )
        .await;
    let id = created["id"].as_str().expect("id");

    let result = exec_ok(
        &f.schema,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(id, json!({}))),
    )
    .await["result"]
        .clone();
    assert_eq!(result["status"], json!("SUCCESS"));
    assert_eq!(result["message"], json!("macro `twice` executed"));
    assert_eq!(result["confirmationRequired"], Value::Null);
    assert!(
        result["payload"]
            .as_object()
            .is_some_and(|p| p.contains_key("chat.post_message")),
        "{result}"
    );
    assert_eq!(f.channel_texts().await, vec!["first", "second"]);
}

/// A step that fails stops the macro: the answer is `ERROR` and later steps do not run.
#[tokio::test]
async fn execute_macro_stops_at_the_first_failing_step() {
    let f = setup().await;
    let created = f
        .create(
            &f.bob,
            "broken",
            json!([
                f.post_step("before"),
                { "commandId": "no.such_command" },
                f.post_step("after"),
            ]),
        )
        .await;
    let id = created["id"].as_str().expect("id");

    let result = exec_ok(
        &f.schema,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(id, json!({}))),
    )
    .await["result"]
        .clone();
    assert_eq!(result["status"], json!("ERROR"));
    assert_eq!(result["message"], json!("macro `broken` failed on a step"));
    assert_eq!(f.channel_texts().await, vec!["before"]);
}

/// A `RESTRICTED` macro answers `CONFIRMATION_REQUIRED` with a challenge and runs nothing; the
/// same call carrying that challenge as proof runs it.
#[tokio::test]
async fn execute_restricted_macro_requires_confirmation() {
    let f = setup().await;
    let created = exec_ok(
        &f.schema,
        f.bob.request(
            &with_fields(CREATE_MACRO),
            json!({ "input": {
                "serverId": f.server_id,
                "name": "careful",
                "steps": [f.post_step("confirmed")],
                "sensitivity": "RESTRICTED",
            } }),
        ),
    )
    .await;
    let id = created["macro"]["id"].as_str().expect("id");

    let first = exec_ok(
        &f.schema,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(id, json!({}))),
    )
    .await["result"]
        .clone();
    assert_eq!(first["status"], json!("CONFIRMATION_REQUIRED"));
    let challenge = &first["confirmationRequired"];
    let token = challenge["token"].as_str().expect("token");
    let args_hash = challenge["argsHash"].as_str().expect("argsHash");
    assert!(challenge["expiresAt"].is_string());
    assert!(f.channel_texts().await.is_empty());

    let proof =
        json!({ "confirmation": { "inBandToken": { "token": token, "argsHash": args_hash } } });
    let second = exec_ok(
        &f.schema,
        f.bob
            .request(EXECUTE_MACRO, f.execute_vars(id, proof.clone())),
    )
    .await["result"]
        .clone();
    assert_eq!(second["status"], json!("SUCCESS"));
    assert_eq!(f.channel_texts().await, vec!["confirmed"]);

    let code = exec_err_code(
        &f.schema,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(id, proof)),
    )
    .await;
    assert_eq!(code, "CONFIRMATION_STALE", "a proof is single use");
}

/// An agent may run only macros marked AI-callable (`FORBIDDEN` otherwise). In an AI-callable
/// macro a read-only step runs for the agent, while a step that writes is refused without
/// a per-step confirmation and the macro answers `ERROR`.
#[tokio::test]
async fn execute_macro_by_an_agent_requires_ai_callable() {
    let f = setup().await;
    let agent = json!({ "initiator": "AGENT" });

    let manual = f.create(&f.bob, "manual", json!([f.list_step()])).await;
    let manual_id = manual["id"].as_str().expect("id");
    let code = exec_err_code(
        &f.schema,
        f.bob
            .request(EXECUTE_MACRO, f.execute_vars(manual_id, agent.clone())),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");

    let reader = f
        .create_ai_callable(&f.bob, "reader", json!([f.list_step()]))
        .await;
    let result = exec_ok(
        &f.schema,
        f.bob
            .request(EXECUTE_MACRO, f.execute_vars(&reader, agent.clone())),
    )
    .await["result"]
        .clone();
    assert_eq!(result["status"], json!("SUCCESS"));
    assert!(
        result["payload"]
            .as_object()
            .is_some_and(|p| p.contains_key("chat.list_channels")),
        "{result}"
    );

    let writer = f
        .create_ai_callable(&f.bob, "writer", json!([f.post_step("by agent")]))
        .await;
    let result = exec_ok(
        &f.schema,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(&writer, agent)),
    )
    .await;
    assert_eq!(result["result"]["status"], json!("ERROR"));
    assert!(f.channel_texts().await.is_empty());
}

/// With the AI surface off, an agent-initiated macro answers `FEATURE_DISABLED`, while the same
/// macro started from the palette still runs.
#[tokio::test]
async fn execute_macro_by_an_agent_needs_the_ai_surface() {
    let f = setup().await;
    let id = f
        .create_ai_callable(&f.bob, "callable", json!([f.post_step("palette")]))
        .await;
    let gated = f
        .ctx
        .build_schema_with_disabled(&[ServerFeature::AiCommands]);

    let code = exec_err_code(
        &gated,
        f.bob.request(
            EXECUTE_MACRO,
            f.execute_vars(&id, json!({ "initiator": "AGENT" })),
        ),
    )
    .await;
    assert_eq!(code, "FEATURE_DISABLED");
    assert!(f.channel_texts().await.is_empty());

    let result = exec_ok(
        &gated,
        f.bob.request(EXECUTE_MACRO, f.execute_vars(&id, json!({}))),
    )
    .await;
    assert_eq!(result["result"]["status"], json!("SUCCESS"));
    assert_eq!(f.channel_texts().await, vec!["palette"]);
}

/// Only the author may run a macro: anyone else, member or not, gets `FORBIDDEN` and nothing
/// runs; an unknown id answers `NOT_FOUND`.
#[tokio::test]
async fn execute_macro_is_author_only() {
    let f = setup().await;
    let created = f
        .create(&f.bob, "private", json!([f.post_step("bob's")]))
        .await;
    let id = created["id"].as_str().expect("id");

    for who in [&f.alice, &f.olaf] {
        let code = exec_err_code(
            &f.schema,
            who.request(EXECUTE_MACRO, f.execute_vars(id, json!({}))),
        )
        .await;
        assert_eq!(code, "FORBIDDEN");
    }
    assert!(f.channel_texts().await.is_empty());

    let missing = uuid::Uuid::now_v7().to_string();
    let code = exec_err_code(
        &f.schema,
        f.bob
            .request(EXECUTE_MACRO, f.execute_vars(&missing, json!({}))),
    )
    .await;
    assert_eq!(code, "NOT_FOUND");
}
