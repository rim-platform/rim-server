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
