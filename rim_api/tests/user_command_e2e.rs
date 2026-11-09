//! Through-the-stack e2e tests for user commands: the ids `createUserCommand` takes, and
//! reading them with `userCommand` and `userCommands`.
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

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, assert_each_refused, exec_err, exec_ok};
use support::roles::Roles;

const CREATE_USER_COMMAND: &str = r#"
mutation Create($input: CreateUserCommandInput!) {
  command: createUserCommand(input: $input) { id }
}
"#;

const UPDATE_USER_COMMAND: &str = r#"
mutation Update($input: UpdateUserCommandInput!) {
  command: updateUserCommand(input: $input) { id }
}
"#;

const USER_COMMAND: &str = r#"
query One($id: String!) {
  command: userCommand(id: $id) { id title description steps { commandId args } isEnabled }
}
"#;

const USER_COMMANDS: &str = r#"
query Mine { commands: userCommands { id isEnabled steps { commandId } } }
"#;

/// The two steps `user.greet` runs, in order.
fn greet_steps() -> Value {
    json!([
        { "commandId": "chat.list_channels", "args": null },
        { "commandId": "chat.post_message", "args": { "body": "hello" } },
    ])
}

struct Fixture {
    schema: AppSchema,
    ws: Roles,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("usercmd").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    Fixture { schema, ws }
}

impl Fixture {
    async fn create_greet(&self, who: &Caller) {
        let input = json!({
            "commandId": "user.greet", "title": "Greet", "description": "Say hello",
            "steps": greet_steps(),
        });
        exec_ok(
            &self.schema,
            who.request(CREATE_USER_COMMAND, json!({ "input": input })),
        )
        .await;
    }

    async fn user_command(&self, who: &Caller, id: &str) -> Value {
        exec_ok(&self.schema, who.request(USER_COMMAND, json!({ "id": id }))).await["command"]
            .clone()
    }

    async fn user_commands(&self, who: &Caller) -> Value {
        exec_ok(&self.schema, who.request(USER_COMMANDS, json!({}))).await["commands"].clone()
    }
}

/// `userCommand` returns the caller's user command with its steps in the order they run and
/// their arguments, as `createUserCommand` saved them.
#[tokio::test]
async fn user_command_returns_the_saved_steps_in_order() {
    let f = setup().await;
    f.create_greet(&f.ws.bob).await;

    assert_eq!(
        f.user_command(&f.ws.bob, "user.greet").await,
        json!({
            "id": "user.greet", "title": "Greet", "description": "Say hello",
            "steps": greet_steps(), "isEnabled": true,
        })
    );
}

/// A disabled user command, which `commands` no longer lists, is still returned by
/// `userCommand` and `userCommands` with its steps, so its owner can edit it.
#[tokio::test]
async fn a_disabled_user_command_is_still_readable() {
    let f = setup().await;
    f.create_greet(&f.ws.bob).await;
    let input = json!({
        "commandId": "user.greet", "title": "Greet", "description": "",
        "steps": [{ "commandId": "chat.list_channels" }], "isEnabled": false,
    });
    exec_ok(
        &f.schema,
        f.ws.bob
            .request(UPDATE_USER_COMMAND, json!({ "input": input })),
    )
    .await;

    let command = f.user_command(&f.ws.bob, "user.greet").await;
    assert_eq!(command["isEnabled"], json!(false));
    assert_eq!(
        command["steps"],
        json!([{ "commandId": "chat.list_channels", "args": null }])
    );
    assert_eq!(
        f.user_commands(&f.ws.bob).await,
        json!([{
            "id": "user.greet", "isEnabled": false,
            "steps": [{ "commandId": "chat.list_channels" }],
        }])
    );
}

/// A user command is private to its owner: another member of the same workspace and a user
/// outside it get null from `userCommand` and do not see it in `userCommands`.
#[tokio::test]
async fn another_users_command_is_not_returned() {
    let f = setup().await;
    f.create_greet(&f.ws.bob).await;

    for who in [&f.ws.carol, &f.ws.alice, &f.ws.olive] {
        assert_eq!(f.user_command(who, "user.greet").await, Value::Null);
        assert_eq!(f.user_commands(who).await, json!([]));
    }
}

/// A built-in command id and an unknown user command id both answer null.
#[tokio::test]
async fn a_builtin_or_unknown_id_answers_null() {
    let f = setup().await;
    f.create_greet(&f.ws.bob).await;

    for id in ["chat.post_message", "user.missing", ""] {
        assert_eq!(f.user_command(&f.ws.bob, id).await, Value::Null, "{id}");
    }
}

/// `userCommands` lists the caller's user commands sorted by id.
#[tokio::test]
async fn user_commands_lists_the_callers_commands_sorted() {
    let f = setup().await;
    for key in ["user.zulu", "user.alpha"] {
        let input = json!({
            "commandId": key, "title": key, "description": "",
            "steps": [{ "commandId": "chat.list_channels" }],
        });
        exec_ok(
            &f.schema,
            f.ws.bob
                .request(CREATE_USER_COMMAND, json!({ "input": input })),
        )
        .await;
    }

    let ids: Vec<Value> = f
        .user_commands(&f.ws.bob)
        .await
        .as_array()
        .expect("user commands")
        .iter()
        .map(|command| command["id"].clone())
        .collect();
    assert_eq!(ids, [json!("user.alpha"), json!("user.zulu")]);
}

/// `createUserCommand` refuses an id with capitals or spaces after `user.` with `VALIDATION`.
#[tokio::test]
async fn create_user_command_refuses_capitals_and_spaces() {
    let f = setup().await;
    let input = json!({
        "commandId": "user.Bad ID", "title": "Bad", "description": "Bad id",
        "steps": greet_steps(),
    });
    let err = exec_err(
        &f.schema,
        f.ws.bob
            .request(CREATE_USER_COMMAND, json!({ "input": input })),
    )
    .await;
    assert_eq!(
        err,
        (
            "VALIDATION".to_string(),
            "a user command id may use only lowercase letters, numbers, dots, dashes or underscores after `user.`".to_string()
        )
    );
    assert_eq!(f.user_commands(&f.ws.bob).await, json!([]));
}

/// Both reads answer `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn user_command_reads_refuse_anonymous_callers() {
    let f = setup().await;
    let calls = [
        (USER_COMMAND, json!({ "id": "user.greet" })),
        (USER_COMMANDS, json!({})),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

const EXECUTE: &str = r#"
mutation Execute($input: ExecuteCommandInput!) {
  result: executeCommand(input: $input) {
    status payload confirmationRequired { token argsHash }
  }
}
"#;

const AI_COMMANDS: &str = r#"
query Ai($context: CommandContextInput) {
  commands(surface: AI, commandContext: $context) { id category aiCallable sourceKind }
}
"#;

const MACRO_STEP_COMMANDS: &str = r#"
query Steps { commands: macroStepCommands { id executionTarget sourceKind } }
"#;

const MACRO_FIELDS: &str = r#"
query One($id: String!) {
  command: userCommand(id: $id) { aiCallable nlDescription intentSamples steps { commandId ask } }
}
"#;

impl Fixture {
    /// Creates the macro `key` for `who` with `steps` and the extra input fields in `extra`.
    async fn create_macro(&self, who: &Caller, key: &str, steps: Value, extra: Value) {
        let mut input = json!({
            "commandId": key, "title": key, "description": "", "steps": steps,
        });
        for (name, value) in extra.as_object().expect("extra fields") {
            input[name] = value.clone();
        }
        exec_ok(
            &self.schema,
            who.request(CREATE_USER_COMMAND, json!({ "input": input })),
        )
        .await;
    }

    /// One step bumping `who`'s own counter `visits` in the workspace.
    fn bump_step(&self) -> Value {
        json!({ "commandId": "state.incr", "args": {
            "server_id": self.ws.server_id, "key": "visits", "scope": "user",
        } })
    }

    /// `executeCommand` of `command_id` by `who` in the workspace, as `initiator`.
    fn execute_input(&self, command_id: &str, initiator: &str) -> Value {
        json!({
            "commandId": command_id,
            "commandContext": { "workspaceId": self.ws.server_id },
            "initiator": initiator,
        })
    }
}

/// An agent-callable macro is offered on the `AI` surface as a `Macro`, and an agent runs it in
/// one call once the person confirms it: the held call runs nothing and the call with the proof
/// runs the step.
#[tokio::test]
async fn the_agent_runs_an_agent_callable_macro_with_a_proof() {
    let f = setup().await;
    let bob = &f.ws.bob;
    f.create_macro(
        bob,
        "user.bump",
        json!([f.bump_step()]),
        json!({ "aiCallable": true, "nlDescription": "Count a visit" }),
    )
    .await;

    let listed = exec_ok(
        &f.schema,
        bob.request(
            AI_COMMANDS,
            json!({ "context": { "workspaceId": f.ws.server_id } }),
        ),
    )
    .await;
    let bump = listed["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .find(|c| c["id"] == json!("user.bump"))
        .cloned()
        .expect("the macro is on the AI surface");
    assert_eq!(
        bump,
        json!({ "id": "user.bump", "category": "Macro", "aiCallable": true, "sourceKind": "USER" })
    );

    let call = f.execute_input("user.bump", "AGENT");
    let held = exec_ok(&f.schema, bob.request(EXECUTE, json!({ "input": call }))).await;
    assert_eq!(held["result"]["status"], json!("CONFIRMATION_REQUIRED"));
    let challenge = &held["result"]["confirmationRequired"];
    let mut proved = call.clone();
    proved["confirmation"] = json!({ "inBandToken": {
        "token": challenge["token"], "argsHash": challenge["argsHash"],
    } });
    let ran = exec_ok(&f.schema, bob.request(EXECUTE, json!({ "input": proved }))).await;
    assert_eq!(ran["result"]["status"], json!("SUCCESS"));
    assert_eq!(ran["result"]["payload"]["state.incr"]["value"], json!(1));
}

/// The agent is refused a macro its owner did not let it run, before anything runs.
#[tokio::test]
async fn the_agent_is_refused_a_macro_it_may_not_run() {
    let f = setup().await;
    let bob = &f.ws.bob;
    f.create_macro(bob, "user.private", json!([f.bump_step()]), json!({}))
        .await;

    let listed = exec_ok(
        &f.schema,
        bob.request(
            AI_COMMANDS,
            json!({ "context": { "workspaceId": f.ws.server_id } }),
        ),
    )
    .await;
    assert!(
        listed["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .all(|c| c["id"] != json!("user.private"))
    );
    let call = f.execute_input("user.private", "AGENT");
    let (code, _) = exec_err(&f.schema, bob.request(EXECUTE, json!({ "input": call }))).await;
    assert_eq!(code, "FORBIDDEN");

    let ran = exec_ok(
        &f.schema,
        bob.request(
            EXECUTE,
            json!({ "input": f.execute_input("user.private", "PALETTE") }),
        ),
    )
    .await;
    assert_eq!(ran["result"]["status"], json!("SUCCESS"));
}

/// A macro with an app step, a plugin step or an asked argument is saved for the person, is
/// refused the agent option, and `executeCommand` tells the caller to run it from the app.
#[tokio::test]
async fn a_macro_with_steps_only_the_app_runs_is_run_from_the_app() {
    let f = setup().await;
    let bob = &f.ws.bob;
    let steps = json!([
        f.bump_step(),
        { "commandId": "workspace.toggle_zen_mode" },
        { "commandId": "plugin.dev.example.poll.poll" },
        { "commandId": "chat.set_topic", "ask": ["topic"] },
    ]);
    let input = json!({
        "commandId": "user.mixed", "title": "Mixed", "description": "", "steps": steps,
        "aiCallable": true,
    });
    let (code, message) = exec_err(
        &f.schema,
        bob.request(CREATE_USER_COMMAND, json!({ "input": input })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(
        message,
        "only a macro whose steps are all commands the AI agent may run, run on the server with every argument fixed, can be run by the AI agent"
    );
    f.create_macro(
        bob,
        "user.mixed",
        steps,
        json!({ "intentSamples": ["mixed bag"] }),
    )
    .await;

    let saved = exec_ok(
        &f.schema,
        bob.request(MACRO_FIELDS, json!({ "id": "user.mixed" })),
    )
    .await;
    assert_eq!(
        saved["command"],
        json!({
            "aiCallable": false, "nlDescription": "", "intentSamples": ["mixed bag"],
            "steps": [
                { "commandId": "state.incr", "ask": [] },
                { "commandId": "workspace.toggle_zen_mode", "ask": [] },
                { "commandId": "plugin.dev.example.poll.poll", "ask": [] },
                { "commandId": "chat.set_topic", "ask": ["topic"] },
            ],
        })
    );

    let (code, message) = exec_err(
        &f.schema,
        bob.request(
            EXECUTE,
            json!({ "input": f.execute_input("user.mixed", "PALETTE") }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(
        message,
        "this macro has steps only the app runs; run it from the app"
    );
}

/// `macroStepCommands` lists the built-ins, the client-run ones included, and the app
/// commands, without the agent-management ones or any macro.
#[tokio::test]
async fn macro_step_commands_list_builtins_and_app_commands() {
    let f = setup().await;
    f.create_macro(&f.ws.bob, "user.bump", json!([f.bump_step()]), json!({}))
        .await;

    let data = exec_ok(&f.schema, f.ws.bob.request(MACRO_STEP_COMMANDS, json!({}))).await;
    let commands = data["commands"].as_array().expect("commands");
    let target = |id: &str| {
        commands
            .iter()
            .find(|c| c["id"] == json!(id))
            .map(|c| c["executionTarget"].clone())
    };
    assert_eq!(target("chat.post_message"), Some(json!("BACKEND")));
    assert_eq!(
        target("workspace.open_search"),
        Some(json!("DESKTOP_LOCAL"))
    );
    assert_eq!(
        target("workspace.toggle_zen_mode"),
        Some(json!("DESKTOP_LOCAL"))
    );
    assert_eq!(target("settings.open.macros"), Some(json!("DESKTOP_LOCAL")));
    assert_eq!(target("agents.approve_request"), None);
    assert_eq!(target("user.bump"), None);
    assert!(
        commands
            .iter()
            .all(|c| c["sourceKind"] == json!("BUILT_IN"))
    );
}

/// `serverInfo.capabilities` lists `account_macros`.
#[tokio::test]
async fn the_server_lists_account_macros() {
    let f = setup().await;
    let data = exec_ok(
        &f.schema,
        anonymous_request("{ serverInfo { capabilities } }", json!({})),
    )
    .await;
    assert!(
        data["serverInfo"]["capabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("account_macros"))
    );
}

/// The agent option is refused on a macro wrapping a built-in the agent may not run itself, so
/// a macro reaches no further than the agent alone; the person keeps the macro without it.
#[tokio::test]
async fn the_agent_option_is_refused_on_a_built_in_the_agent_may_not_run() {
    let f = setup().await;
    let alice = &f.ws.alice;
    let steps = json!([{ "commandId": "admin.invite_member", "args": {
        "server_id": f.ws.server_id,
    } }]);
    let input = json!({
        "commandId": "user.invite", "title": "Invite", "description": "", "steps": steps,
        "aiCallable": true,
    });
    let (code, message) = exec_err(
        &f.schema,
        alice.request(CREATE_USER_COMMAND, json!({ "input": input })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(
        message,
        "only a macro whose steps are all commands the AI agent may run, run on the server with every argument fixed, can be run by the AI agent"
    );

    f.create_macro(alice, "user.invite", steps, json!({})).await;
    let update = json!({
        "commandId": "user.invite", "title": "Invite", "description": "",
        "steps": [{ "commandId": "admin.invite_member" }], "isEnabled": true, "aiCallable": true,
    });
    let (code, _) = exec_err(
        &f.schema,
        alice.request(UPDATE_USER_COMMAND, json!({ "input": update })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    let (code, _) = exec_err(
        &f.schema,
        alice.request(
            EXECUTE,
            json!({ "input": f.execute_input("user.invite", "AGENT") }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");
}
