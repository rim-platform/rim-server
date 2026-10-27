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
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err,
    exec_err_code, exec_ok, merged,
};
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

/// `$operation` followed by the fragment selecting every `MacroDef` field.
macro_rules! with_macro_fields {
    ($operation:literal) => {
        concat!(
            $operation,
            r#"
fragment MacroFields on MacroDef {
  id serverId userId name description steps { commandId args } aiCallable nlDescription
  intentSamples sensitivity createdAt
}
"#
        )
    };
}

const CREATE_MACRO: &str = with_macro_fields!(
    "mutation Create($input: CreateMacroInput!) { macro: createMacro(input: $input) { ...MacroFields } }"
);

const UPDATE_MACRO: &str = with_macro_fields!(
    "mutation Update($input: UpdateMacroInput!) { macro: updateMacro(input: $input) { ...MacroFields } }"
);

const MY_MACROS: &str = with_macro_fields!(
    "query Mine($serverId: ID!) { macros: myMacros(serverId: $serverId) { ...MacroFields } }"
);

const MACRO_BY_NAME: &str = with_macro_fields!(
    "query ByName($serverId: ID!, $name: String!) {
       macro: macroByName(serverId: $serverId, name: $name) { ...MacroFields }
     }"
);

const DELETE_MACRO: &str = "mutation Delete($macroId: ID!) { deleteMacro(macroId: $macroId) }";

const EXECUTE_MACRO: &str = r#"
mutation Execute($input: ExecuteMacroInput!) {
  result: executeMacro(input: $input) {
    status message payload confirmationRequired { token argsHash expiresAt }
  }
}
"#;

const MESSAGES_QUERY: &str = r#"
query Messages($serverId: ID!, $conversationId: ID!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: AFTER, limit: 50,
                 includeDeleted: false) { items { text } }
}
"#;

const JOIN_CHANNEL: &str = r#"
mutation Join($serverId: ID!, $conversationId: ID!) {
  conversation: joinChannel(serverId: $serverId, conversationId: $conversationId) { id }
}
"#;

const IS_MEMBER_QUERY: &str = r#"
query IsMember($serverId: ID!, $conversationId: ID!) {
  conversation(serverId: $serverId, conversationId: $conversationId) { isMember }
}
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    ws: Roles,
    /// Public channel alice created; the post steps write into it.
    channel_id: String,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    ctx.seed_capabilities().await;
    let ws = ctx.roles("macro").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let channel_id = create_channel(&schema, &ws.alice, &ws.server_id, "general", false).await;
    Fixture {
        ctx,
        schema,
        ws,
        channel_id,
    }
}

/// `executeMacro` input fields marking the run as started by an AI agent.
fn agent() -> Value {
    json!({ "initiator": "AGENT" })
}

fn id(value: &Value) -> &str {
    value["id"].as_str().expect("macro id")
}

impl Fixture {
    /// A step posting `text` into the fixture channel.
    fn post_step(&self, text: &str) -> Value {
        json!({
            "commandId": "chat.post_message",
            "args": {
                "server_id": self.ws.server_id,
                "conversation_id": self.channel_id,
                "text": text,
            },
        })
    }

    /// A step removing bob from the fixture channel: a dangerous command of the `Mutating`
    /// tier that a member may run on themselves.
    fn leave_step(&self) -> Value {
        json!({
            "commandId": "chat.remove_conversation_member",
            "args": {
                "server_id": self.ws.server_id,
                "conversation_id": self.channel_id,
                "user_id": self.ws.bob.user_id.to_string(),
            },
        })
    }

    /// `count` steps posting their own index.
    fn post_steps(&self, count: usize) -> Value {
        Value::Array((0..count).map(|i| self.post_step(&i.to_string())).collect())
    }

    /// A read-only step listing the workspace's channels.
    fn list_step(&self) -> Value {
        json!({ "commandId": "chat.list_channels", "args": { "server_id": self.ws.server_id } })
    }

    /// `createMacro` variables: `name` and `steps` in the fixture workspace, plus `extra` fields.
    fn create_vars(&self, name: &str, steps: Value, extra: Value) -> Value {
        let input = json!({ "serverId": self.ws.server_id, "name": name, "steps": steps });
        json!({ "input": merged(input, extra) })
    }

    async fn create_with(&self, who: &Caller, name: &str, steps: Value, extra: Value) -> Value {
        let vars = self.create_vars(name, steps, extra);
        exec_ok(&self.schema, who.request(CREATE_MACRO, vars)).await["macro"].clone()
    }

    async fn create(&self, who: &Caller, name: &str, steps: Value) -> Value {
        self.create_with(who, name, steps, json!({})).await
    }

    async fn create_code(&self, who: &Caller, name: &str, steps: Value) -> String {
        let vars = self.create_vars(name, steps, json!({}));
        exec_err_code(&self.schema, who.request(CREATE_MACRO, vars)).await
    }

    fn update_vars(&self, macro_id: &str, name: &str, steps: Value) -> Value {
        json!({ "input": { "macroId": macro_id, "name": name, "steps": steps } })
    }

    async fn update_code(&self, who: &Caller, macro_id: &str, name: &str, steps: Value) -> String {
        let vars = self.update_vars(macro_id, name, steps);
        exec_err_code(&self.schema, who.request(UPDATE_MACRO, vars)).await
    }

    async fn delete_code(&self, who: &Caller, macro_id: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(DELETE_MACRO, json!({ "macroId": macro_id })),
        )
        .await
    }

    async fn mine(&self, who: &Caller) -> Vec<Value> {
        let vars = json!({ "serverId": self.ws.server_id });
        let data = exec_ok(&self.schema, who.request(MY_MACROS, vars)).await;
        data["macros"].as_array().expect("macros").clone()
    }

    fn by_name_vars(&self, name: &str) -> Value {
        json!({ "serverId": self.ws.server_id, "name": name })
    }

    async fn by_name(&self, who: &Caller, name: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(MACRO_BY_NAME, self.by_name_vars(name)),
        )
        .await;
        data["macro"].clone()
    }

    /// `executeMacro` variables in the fixture workspace, plus `extra` input fields.
    fn execute_vars(&self, macro_id: &str, extra: Value) -> Value {
        let input = json!({
            "macroId": macro_id,
            "commandContext": { "workspaceId": self.ws.server_id },
        });
        json!({ "input": merged(input, extra) })
    }

    /// Run the macro as bob on `schema` and return the `CommandResult`.
    async fn execute_on(&self, schema: &AppSchema, macro_id: &str, extra: Value) -> Value {
        let vars = self.execute_vars(macro_id, extra);
        exec_ok(schema, self.ws.bob.request(EXECUTE_MACRO, vars)).await["result"].clone()
    }

    async fn execute(&self, macro_id: &str, extra: Value) -> Value {
        self.execute_on(&self.schema, macro_id, extra).await
    }

    async fn execute_code(&self, who: &Caller, macro_id: &str, extra: Value) -> String {
        let vars = self.execute_vars(macro_id, extra);
        exec_err_code(&self.schema, who.request(EXECUTE_MACRO, vars)).await
    }

    /// Whether bob is a member of the fixture channel.
    async fn bob_is_member(&self) -> bool {
        let vars = json!({ "serverId": self.ws.server_id, "conversationId": self.channel_id });
        let data = exec_ok(&self.schema, self.ws.bob.request(IS_MEMBER_QUERY, vars)).await;
        data["conversation"]["isMember"]
            .as_bool()
            .expect("isMember")
    }

    /// The texts of the fixture channel's messages, oldest first.
    async fn channel_texts(&self) -> Vec<String> {
        let vars = json!({ "serverId": self.ws.server_id, "conversationId": self.channel_id });
        let data = exec_ok(&self.schema, self.ws.alice.request(MESSAGES_QUERY, vars)).await;
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
    let missing = unknown_id();
    let steps = json!([f.post_step("x")]);
    let calls = [
        (CREATE_MACRO, f.create_vars("m", steps.clone(), json!({}))),
        (UPDATE_MACRO, f.update_vars(&missing, "m", steps)),
        (DELETE_MACRO, json!({ "macroId": missing })),
        (EXECUTE_MACRO, f.execute_vars(&missing, json!({}))),
        (MY_MACROS, json!({ "serverId": f.ws.server_id })),
        (MACRO_BY_NAME, f.by_name_vars("m")),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// A user outside the workspace cannot create, look up by name or list macros there.
#[tokio::test]
async fn macro_roots_refuse_non_members() {
    let f = setup().await;
    f.create(&f.ws.alice, "greet", json!([f.post_step("hi")]))
        .await;
    let calls = [
        (
            CREATE_MACRO,
            f.create_vars("intrude", json!([f.post_step("x")]), json!({})),
        ),
        (MACRO_BY_NAME, f.by_name_vars("greet")),
        (MY_MACROS, json!({ "serverId": f.ws.server_id })),
    ];
    assert_each_refused_as(&f.schema, &f.ws.olive, &calls, "FORBIDDEN").await;
}

/// `createMacro` returns every field of the new macro, with the defaults for omitted inputs,
/// and the macro is then listed and found by name for its author only.
#[tokio::test]
async fn create_macro_returns_the_macro_and_lists_it_for_its_author() {
    let f = setup().await;
    let before = chrono::Utc::now();
    let steps = json!([f.post_step("standup"), { "commandId": "chat.mark_as_read" }]);
    let created = f
        .create_with(
            &f.ws.bob,
            "  standup  ",
            steps,
            json!({
                "description": "post the standup note",
                "aiCallable": true,
                "nlDescription": "posts the standup note",
                "intentSamples": ["standup please"],
                "sensitivity": "MUTATING",
            }),
        )
        .await;

    assert_eq!(created["serverId"], json!(f.ws.server_id));
    assert_eq!(created["userId"], json!(f.ws.bob.user_id.to_string()));
    assert_eq!(created["name"], json!("standup"), "the name is trimmed");
    assert_eq!(created["description"], json!("post the standup note"));
    assert_eq!(
        created["steps"],
        json!([f.post_step("standup"), { "commandId": "chat.mark_as_read", "args": null }])
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

    let defaults = f
        .create(&f.ws.bob, "plain", json!([f.post_step("x")]))
        .await;
    assert_eq!(defaults["description"], Value::Null);
    assert_eq!(defaults["aiCallable"], json!(false));
    assert_eq!(defaults["nlDescription"], json!(""));
    assert_eq!(defaults["intentSamples"], json!([]));
    assert_eq!(defaults["sensitivity"], json!("SAFE"));

    let mut names: Vec<_> = f
        .mine(&f.ws.bob)
        .await
        .iter()
        .map(|m| m["name"].to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec![r#""plain""#, r#""standup""#]);
    assert_eq!(f.by_name(&f.ws.bob, "standup").await, created);

    assert!(f.mine(&f.ws.alice).await.is_empty(), "macros are per user");
    assert_eq!(f.by_name(&f.ws.alice, "standup").await, Value::Null);
    assert_eq!(f.by_name(&f.ws.bob, "missing").await, Value::Null);
}

/// `createMacro` refuses a blank or over-long name, no steps, more than twenty steps, a step
/// that calls another macro or a user command, and a step whose arguments pass 4 KiB, with
/// `VALIDATION`.
#[tokio::test]
async fn create_macro_validates_name_and_steps() {
    let f = setup().await;
    let cases = [
        ("   ".to_string(), f.post_steps(1)),
        ("n".repeat(129), f.post_steps(1)),
        ("empty".to_string(), json!([])),
        ("long".to_string(), f.post_steps(21)),
        (
            "nested".to_string(),
            json!([{ "commandId": "macro.anything" }]),
        ),
        (
            "composed".to_string(),
            json!([{ "commandId": "user.many_steps" }]),
        ),
        ("bulky".to_string(), json!([f.post_step(&"x".repeat(4096))])),
    ];
    for (name, steps) in cases {
        assert_eq!(
            f.create_code(&f.ws.bob, &name, steps).await,
            "VALIDATION",
            "{name}"
        );
    }

    f.create(&f.ws.bob, &"n".repeat(128), f.post_steps(20))
        .await;
    assert_eq!(f.mine(&f.ws.bob).await.len(), 1);
}

/// `createMacro` refuses a step naming no builtin command with `VALIDATION`, naming the id,
/// and saves nothing.
#[tokio::test]
async fn create_macro_refuses_an_unknown_command() {
    let f = setup().await;
    let steps = json!([f.post_step("x"), { "commandId": "chat.mark_read" }]);
    let vars = f.create_vars("unknown", steps, json!({}));
    let (code, message) = exec_err(&f.schema, f.ws.bob.request(CREATE_MACRO, vars)).await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(message, "Unknown command: chat.mark_read");
    assert!(f.mine(&f.ws.bob).await.is_empty());
}

/// A second macro with the same name for the same user in the same workspace answers
/// `CONFLICT`; another user may reuse the name.
#[tokio::test]
async fn create_macro_refuses_a_duplicate_name() {
    let f = setup().await;
    f.create(&f.ws.bob, "dup", f.post_steps(1)).await;
    assert_eq!(
        f.create_code(&f.ws.bob, "dup", f.post_steps(1)).await,
        "CONFLICT"
    );
    f.create(&f.ws.alice, "dup", f.post_steps(1)).await;
}

/// A user may keep at most fifty macros per workspace; the fifty-first answers `VALIDATION`.
#[tokio::test]
async fn create_macro_caps_macros_per_user() {
    let f = setup().await;
    for i in 0..50 {
        f.create(&f.ws.bob, &format!("m{i}"), f.post_steps(1)).await;
    }
    assert_eq!(
        f.create_code(&f.ws.bob, "one_too_many", f.post_steps(1))
            .await,
        "VALIDATION"
    );
    f.create(&f.ws.alice, "unaffected", f.post_steps(1)).await;
}

/// `macroByName` refuses a name over 128 characters with `VALIDATION`.
#[tokio::test]
async fn macro_by_name_refuses_an_over_long_name() {
    let f = setup().await;
    let request =
        f.ws.bob
            .request(MACRO_BY_NAME, f.by_name_vars(&"n".repeat(129)));
    assert_eq!(exec_err_code(&f.schema, request).await, "VALIDATION");
}

/// `updateMacro` replaces the author's macro and returns it; anyone else gets `FORBIDDEN`, an
/// unknown id `NOT_FOUND`, and the same name and step rules as creation apply.
#[tokio::test]
async fn update_macro_is_author_only_and_validated() {
    let f = setup().await;
    let created = f.create(&f.ws.bob, "old", f.post_steps(1)).await;
    let id = id(&created);

    let vars = json!({ "input": {
        "macroId": id,
        "name": "new",
        "steps": f.post_steps(2),
        "description": "changed",
        "aiCallable": true,
        "sensitivity": "RESTRICTED",
    } });
    let updated = exec_ok(&f.schema, f.ws.bob.request(UPDATE_MACRO, vars)).await["macro"].clone();
    assert_eq!(updated["id"], json!(id));
    assert_eq!(updated["name"], json!("new"));
    assert_eq!(updated["description"], json!("changed"));
    assert_eq!(updated["steps"], f.post_steps(2));
    assert_eq!(updated["aiCallable"], json!(true));
    assert_eq!(updated["sensitivity"], json!("RESTRICTED"));
    assert_eq!(updated["createdAt"], created["createdAt"]);
    assert_eq!(f.by_name(&f.ws.bob, "old").await, Value::Null);
    assert_eq!(f.by_name(&f.ws.bob, "new").await, updated);

    for who in [&f.ws.alice, &f.ws.olive] {
        assert_eq!(
            f.update_code(who, id, "stolen", f.post_steps(1)).await,
            "FORBIDDEN"
        );
    }
    let missing = unknown_id();
    assert_eq!(
        f.update_code(&f.ws.bob, &missing, "x", f.post_steps(1))
            .await,
        "NOT_FOUND"
    );
    for (name, steps) in [
        (" ", f.post_steps(1)),
        ("new", json!([])),
        ("new", json!([{ "commandId": "macro.other" }])),
    ] {
        assert_eq!(
            f.update_code(&f.ws.bob, id, name, steps).await,
            "VALIDATION"
        );
    }
    assert_eq!(f.by_name(&f.ws.bob, "new").await, updated);
}

/// `deleteMacro` removes the author's macro and answers `true`; anyone else gets `FORBIDDEN`
/// and a deleted macro `NOT_FOUND`.
#[tokio::test]
async fn delete_macro_is_author_only() {
    let f = setup().await;
    let created = f.create(&f.ws.bob, "gone", f.post_steps(1)).await;
    let id = id(&created);

    for who in [&f.ws.alice, &f.ws.olive] {
        assert_eq!(f.delete_code(who, id).await, "FORBIDDEN");
    }
    assert_eq!(f.mine(&f.ws.bob).await.len(), 1);

    let data = exec_ok(
        &f.schema,
        f.ws.bob.request(DELETE_MACRO, json!({ "macroId": id })),
    )
    .await;
    assert_eq!(data["deleteMacro"], json!(true));
    assert!(f.mine(&f.ws.bob).await.is_empty());
    assert_eq!(f.delete_code(&f.ws.bob, id).await, "NOT_FOUND");
}

/// `executeMacro` runs every step as the caller and answers `SUCCESS` with each step's payload
/// keyed by its command id.
#[tokio::test]
async fn execute_macro_runs_every_step() {
    let f = setup().await;
    let steps = json!([f.post_step("first"), f.post_step("second")]);
    let created = f.create(&f.ws.bob, "twice", steps).await;

    let result = f.execute(id(&created), json!({})).await;
    assert_eq!(result["status"], json!("SUCCESS"));
    assert_eq!(result["message"], json!("macro `twice` executed"));
    assert_eq!(result["confirmationRequired"], Value::Null);
    assert!(
        result["payload"].get("chat.post_message").is_some(),
        "{result}"
    );
    assert_eq!(f.channel_texts().await, vec!["first", "second"]);
}

/// A step that fails stops the macro: the answer is `ERROR`, its message names the step and
/// its error, and later steps do not run.
#[tokio::test]
async fn execute_macro_stops_at_the_first_failing_step() {
    let f = setup().await;
    let steps = json!([
        f.post_step("before"),
        {
            "commandId": "chat.post_message",
            "args": { "server_id": f.ws.server_id, "conversation_id": f.channel_id },
        },
        f.post_step("after"),
    ]);
    let created = f.create(&f.ws.bob, "broken", steps).await;

    let result = f.execute(id(&created), json!({})).await;
    assert_eq!(result["status"], json!("ERROR"));
    assert_eq!(
        result["message"],
        json!("Step 2 (Post Message) failed: missing string arg `text`")
    );
    assert_eq!(f.channel_texts().await, vec!["before"]);
}

/// A `RESTRICTED` macro answers `CONFIRMATION_REQUIRED` with a challenge and runs nothing; the
/// same call carrying that challenge as proof runs it, once.
#[tokio::test]
async fn execute_restricted_macro_requires_confirmation() {
    let f = setup().await;
    let restricted = json!({ "sensitivity": "RESTRICTED" });
    let created = f
        .create_with(
            &f.ws.bob,
            "careful",
            json!([f.post_step("confirmed")]),
            restricted,
        )
        .await;
    let id = id(&created);

    let first = f.execute(id, json!({})).await;
    assert_eq!(first["status"], json!("CONFIRMATION_REQUIRED"));
    let challenge = &first["confirmationRequired"];
    assert!(challenge["expiresAt"].is_string());
    assert!(f.channel_texts().await.is_empty());

    let proof = json!({ "confirmation": { "inBandToken": {
        "token": challenge["token"], "argsHash": challenge["argsHash"],
    } } });
    let second = f.execute(id, proof.clone()).await;
    assert_eq!(second["status"], json!("SUCCESS"));
    assert_eq!(f.channel_texts().await, vec!["confirmed"]);

    assert_eq!(
        f.execute_code(&f.ws.bob, id, proof).await,
        "CONFIRMATION_STALE",
        "a proof is single use"
    );
}

/// A macro with a step that is dangerous but only `MUTATING` answers `CONFIRMATION_REQUIRED`
/// up front and runs none of its steps; the same call carrying that challenge as proof runs
/// every step, the dangerous one included, with that one confirmation.
#[tokio::test]
async fn execute_macro_with_a_dangerous_step_asks_for_confirmation_once() {
    let f = setup().await;
    let joined = json!({ "serverId": f.ws.server_id, "conversationId": f.channel_id });
    exec_ok(&f.schema, f.ws.bob.request(JOIN_CHANNEL, joined)).await;
    assert!(f.bob_is_member().await);

    let created = f
        .create(
            &f.ws.bob,
            "leave",
            json!([f.post_step("leaving"), f.leave_step()]),
        )
        .await;
    let id = id(&created);

    let first = f.execute(id, json!({})).await;
    assert_eq!(first["status"], json!("CONFIRMATION_REQUIRED"), "{first}");
    assert_eq!(
        first["message"],
        json!("macro requires confirmation due to step `chat.remove_conversation_member`")
    );
    let challenge = &first["confirmationRequired"];
    assert!(challenge["expiresAt"].is_string());
    assert!(f.channel_texts().await.is_empty());
    assert!(
        f.bob_is_member().await,
        "nothing runs before the confirmation"
    );

    let proof = json!({ "confirmation": { "inBandToken": {
        "token": challenge["token"], "argsHash": challenge["argsHash"],
    } } });
    let second = f.execute(id, proof).await;
    assert_eq!(second["status"], json!("SUCCESS"), "{second}");
    assert_eq!(f.channel_texts().await, vec!["leaving"]);
    assert!(!f.bob_is_member().await, "the dangerous step ran");
}

/// An agent may run only macros marked AI-callable (`FORBIDDEN` otherwise). In an AI-callable
/// macro a read-only step runs for the agent without a confirmation, while a macro with a
/// step that writes answers `CONFIRMATION_REQUIRED` up front and runs nothing.
#[tokio::test]
async fn execute_macro_by_an_agent_requires_ai_callable() {
    let f = setup().await;
    let ai_callable = json!({ "aiCallable": true });

    let manual = f.create(&f.ws.bob, "manual", json!([f.list_step()])).await;
    assert_eq!(
        f.execute_code(&f.ws.bob, id(&manual), agent()).await,
        "FORBIDDEN"
    );

    let reader = f
        .create_with(
            &f.ws.bob,
            "reader",
            json!([f.list_step()]),
            ai_callable.clone(),
        )
        .await;
    let result = f.execute(id(&reader), agent()).await;
    assert_eq!(result["status"], json!("SUCCESS"));
    assert!(
        result["payload"].get("chat.list_channels").is_some(),
        "{result}"
    );

    let writer = f
        .create_with(
            &f.ws.bob,
            "writer",
            json!([f.post_step("by agent")]),
            ai_callable,
        )
        .await;
    let result = f.execute(id(&writer), agent()).await;
    assert_eq!(result["status"], json!("CONFIRMATION_REQUIRED"), "{result}");
    assert!(result["confirmationRequired"]["token"].is_string());
    assert!(f.channel_texts().await.is_empty());
}

/// With the AI surface off, a macro started from the palette still runs.
#[tokio::test]
async fn execute_macro_from_the_palette_runs_while_the_ai_surface_is_off() {
    let f = setup().await;
    let created = f
        .create(&f.ws.bob, "palette", json!([f.post_step("palette")]))
        .await;
    let gated = f
        .ctx
        .build_schema_with_disabled(&[ServerFeature::AiCommands]);

    let result = f.execute_on(&gated, id(&created), json!({})).await;
    assert_eq!(result["status"], json!("SUCCESS"));
    assert_eq!(f.channel_texts().await, vec!["palette"]);
}

/// Only the author may run a macro: anyone else, member or not, gets `FORBIDDEN` and nothing
/// runs; an unknown id answers `NOT_FOUND`.
#[tokio::test]
async fn execute_macro_is_author_only() {
    let f = setup().await;
    let created = f
        .create(&f.ws.bob, "private", json!([f.post_step("bob's")]))
        .await;

    for who in [&f.ws.alice, &f.ws.olive] {
        assert_eq!(
            f.execute_code(who, id(&created), json!({})).await,
            "FORBIDDEN"
        );
    }
    assert!(f.channel_texts().await.is_empty());

    let missing = unknown_id();
    assert_eq!(
        f.execute_code(&f.ws.bob, &missing, json!({})).await,
        "NOT_FOUND"
    );
}
