//! Through-the-stack e2e tests for the keybind GraphQL contract: `createKeybind`,
//! `updateKeybind` and `deleteKeybind`.
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
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
    merged,
};
use support::roles::Roles;
use support::{TestCtx, unknown_id};

const CREATE_KEYBIND: &str = r#"
mutation Create($input: CreateKeybindInput!) {
  keybind: createKeybind(input: $input) {
    id source scope workspaceId keystroke commandId context presetArgs isEnabled
  }
}
"#;

const UPDATE_KEYBIND: &str = r#"
mutation Update($input: UpdateKeybindInput!) {
  keybind: updateKeybind(input: $input) {
    id source scope workspaceId keystroke commandId context presetArgs isEnabled
  }
}
"#;

const DELETE_KEYBIND: &str =
    "mutation Delete($input: DeleteKeybindInput!) { deleted: deleteKeybind(input: $input) }";

const RESOLVED_KEYBINDS: &str = r#"
query Resolved($workspaceId: ID) {
  keybinds: resolvedKeybinds(workspaceId: $workspaceId) { id }
}
"#;

/// A command the keybind picker offers.
const BINDABLE: &str = "workspace.toggle_sidebar";

struct Fixture {
    schema: AppSchema,
    ws: Roles,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("keybind").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    Fixture { schema, ws }
}

fn id(keybind: &Value) -> &str {
    keybind["id"].as_str().expect("keybind id")
}

impl Fixture {
    /// `createKeybind` variables for a user keybind on `Ctrl+B` in the global context, with
    /// `extra` input fields.
    fn create_vars(&self, extra: Value) -> Value {
        let input = json!({
            "scope": "USER", "keystroke": "Ctrl+B", "commandId": BINDABLE, "context": "GLOBAL",
        });
        json!({ "input": merged(input, extra) })
    }

    /// `extra` fields making a create input a workspace keybind in the fixture workspace.
    fn workspace(&self) -> Value {
        json!({ "scope": "WORKSPACE", "workspaceId": self.ws.server_id })
    }

    async fn create(&self, who: &Caller, extra: Value) -> Value {
        let vars = self.create_vars(extra);
        exec_ok(&self.schema, who.request(CREATE_KEYBIND, vars)).await["keybind"].clone()
    }

    async fn create_code(&self, who: &Caller, extra: Value) -> String {
        let vars = self.create_vars(extra);
        exec_err_code(&self.schema, who.request(CREATE_KEYBIND, vars)).await
    }

    /// `updateKeybind` variables rebinding `keybind_id` to `Ctrl+J` in the composer, with
    /// `extra` input fields.
    fn update_vars(&self, keybind_id: &str, extra: Value) -> Value {
        let input = json!({
            "keybindId": keybind_id, "keystroke": "Ctrl+J", "commandId": BINDABLE,
            "context": "COMPOSER", "isEnabled": true,
        });
        json!({ "input": merged(input, extra) })
    }

    async fn update_code(&self, who: &Caller, keybind_id: &str, extra: Value) -> String {
        let vars = self.update_vars(keybind_id, extra);
        exec_err_code(&self.schema, who.request(UPDATE_KEYBIND, vars)).await
    }

    fn delete_vars(keybind_id: &str) -> Value {
        json!({ "input": { "keybindId": keybind_id } })
    }

    async fn delete_code(&self, who: &Caller, keybind_id: &str) -> String {
        let vars = Self::delete_vars(keybind_id);
        exec_err_code(&self.schema, who.request(DELETE_KEYBIND, vars)).await
    }

    /// Whether `keybind_id` is among the keybinds `who` resolves in the fixture workspace.
    async fn resolves(&self, who: &Caller, keybind_id: &str) -> bool {
        let vars = json!({ "workspaceId": self.ws.server_id });
        let data = exec_ok(&self.schema, who.request(RESOLVED_KEYBINDS, vars)).await;
        data["keybinds"]
            .as_array()
            .expect("keybinds")
            .iter()
            .any(|k| k["id"] == json!(keybind_id))
    }
}

/// Every keybind mutation answers `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn keybind_mutations_refuse_anonymous_callers() {
    let f = setup().await;
    let missing = unknown_id();
    let calls = [
        (CREATE_KEYBIND, f.create_vars(json!({}))),
        (UPDATE_KEYBIND, f.update_vars(&missing, json!({}))),
        (DELETE_KEYBIND, Fixture::delete_vars(&missing)),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// Only the workspace owner binds, rebinds and unbinds workspace keybinds; moderators, members,
/// guests and outsiders get `FORBIDDEN`. Each of them may still keep keybinds of their own.
#[tokio::test]
async fn only_the_owner_manages_workspace_keybinds() {
    let f = setup().await;
    let shared = f.create(&f.ws.alice, f.workspace()).await;
    let calls = [
        (CREATE_KEYBIND, f.create_vars(f.workspace())),
        (UPDATE_KEYBIND, f.update_vars(id(&shared), json!({}))),
        (DELETE_KEYBIND, Fixture::delete_vars(id(&shared))),
    ];
    for who in [&f.ws.mona, &f.ws.bob, &f.ws.gina, &f.ws.olive] {
        assert_each_refused_as(&f.schema, who, &calls, "FORBIDDEN").await;
        let own = f.create(who, json!({})).await;
        assert_eq!(own["scope"], json!("USER"));
    }
    assert!(f.resolves(&f.ws.bob, id(&shared)).await);
}

/// `createKeybind` returns every field of the new keybind, with the keystroke trimmed and the
/// keybind enabled; a workspace keybind carries its workspace and resolves for every member.
#[tokio::test]
async fn create_keybind_returns_the_keybind() {
    let f = setup().await;
    let personal = f
        .create(
            &f.ws.bob,
            json!({ "keystroke": "  Ctrl+Shift+B  ", "presetArgs": { "side": "left" } }),
        )
        .await;
    assert!(uuid::Uuid::parse_str(id(&personal)).is_ok());
    assert_eq!(
        merged(personal.clone(), json!({ "id": null })),
        json!({
            "id": null, "source": "USER", "scope": "USER", "workspaceId": null,
            "keystroke": "Ctrl+Shift+B", "commandId": BINDABLE, "context": "GLOBAL",
            "presetArgs": { "side": "left" }, "isEnabled": true,
        })
    );
    assert!(f.resolves(&f.ws.bob, id(&personal)).await);
    assert!(
        !f.resolves(&f.ws.carol, id(&personal)).await,
        "a user keybind is private"
    );

    let shared = f
        .create(
            &f.ws.alice,
            merged(f.workspace(), json!({ "context": "PANEL" })),
        )
        .await;
    assert_eq!(shared["source"], json!("WORKSPACE"));
    assert_eq!(shared["scope"], json!("WORKSPACE"));
    assert_eq!(shared["workspaceId"], json!(f.ws.server_id));
    assert_eq!(shared["context"], json!("PANEL"));
    assert_eq!(shared["presetArgs"], Value::Null);
    assert!(f.resolves(&f.ws.carol, id(&shared)).await);
}

/// `createKeybind` refuses a malformed keystroke, a command the picker does not offer, a
/// workspace keybind without a workspace and a malformed workspace id with `VALIDATION`, and a
/// second keybind on the same keystroke and context with `CONFLICT`.
#[tokio::test]
async fn create_keybind_rejects_invalid_input() {
    let f = setup().await;
    for (extra, why) in [
        (json!({ "keystroke": "   " }), "blank keystroke"),
        (json!({ "keystroke": "Hyper+B" }), "unknown modifier"),
        (json!({ "keystroke": "Ctrl+" }), "no key"),
        (
            json!({ "keystroke": "K".repeat(51) }),
            "over-long keystroke",
        ),
        (json!({ "commandId": "chat.post_message" }), "not bindable"),
        (json!({ "commandId": "no.such_command" }), "unknown command"),
        (
            json!({ "scope": "WORKSPACE" }),
            "workspace keybind without a workspace",
        ),
        (
            json!({ "workspaceId": "nope", "scope": "WORKSPACE" }),
            "malformed workspace id",
        ),
        (
            json!({ "presetArgs": { "blob": "x".repeat(4096) } }),
            "over-long preset arguments",
        ),
    ] {
        assert_eq!(f.create_code(&f.ws.bob, extra).await, "VALIDATION", "{why}");
    }

    f.create(&f.ws.bob, json!({})).await;
    assert_eq!(
        f.create_code(&f.ws.bob, json!({ "keystroke": " Ctrl+B " }))
            .await,
        "CONFLICT"
    );
    let other_context = f.create(&f.ws.bob, json!({ "context": "COMPOSER" })).await;
    assert_eq!(other_context["keystroke"], json!("Ctrl+B"));
    let other_user = f.create(&f.ws.carol, json!({})).await;
    assert_eq!(other_user["keystroke"], json!("Ctrl+B"));
}

/// `updateKeybind` replaces the keystroke, command, context, preset arguments and enabled flag
/// of its owner's keybind and keeps its id and scope; anyone else gets `FORBIDDEN`, an unknown
/// id `NOT_FOUND`, and the creation rules apply.
#[tokio::test]
async fn update_keybind_replaces_the_owners_keybind() {
    let f = setup().await;
    let keybind = f.create(&f.ws.bob, json!({})).await;
    let vars = f.update_vars(
        id(&keybind),
        json!({ "keystroke": " Alt+J ", "presetArgs": [1, 2], "isEnabled": false }),
    );
    let updated =
        exec_ok(&f.schema, f.ws.bob.request(UPDATE_KEYBIND, vars)).await["keybind"].clone();
    assert_eq!(
        updated,
        json!({
            "id": id(&keybind), "source": "USER", "scope": "USER", "workspaceId": null,
            "keystroke": "Alt+J", "commandId": BINDABLE, "context": "COMPOSER",
            "presetArgs": [1, 2], "isEnabled": false,
        })
    );

    for who in [&f.ws.carol, &f.ws.alice, &f.ws.olive] {
        assert_eq!(
            f.update_code(who, id(&keybind), json!({})).await,
            "FORBIDDEN"
        );
    }
    assert_eq!(
        f.update_code(&f.ws.bob, &unknown_id(), json!({})).await,
        "NOT_FOUND"
    );
    for extra in [
        json!({ "keystroke": "Hyper+J" }),
        json!({ "commandId": "chat.post_message" }),
    ] {
        assert_eq!(
            f.update_code(&f.ws.bob, id(&keybind), extra).await,
            "VALIDATION"
        );
    }
    assert_eq!(
        f.update_code(&f.ws.bob, "nope", json!({})).await,
        "VALIDATION",
        "malformed id"
    );

    f.create(&f.ws.bob, json!({ "keystroke": "Ctrl+K" })).await;
    let code = f
        .update_code(
            &f.ws.bob,
            id(&keybind),
            json!({ "keystroke": "Ctrl+K", "context": "GLOBAL" }),
        )
        .await;
    assert_eq!(code, "CONFLICT");
}

/// `deleteKeybind` removes its owner's keybind and answers `true`; anyone else gets `FORBIDDEN`
/// and a deleted or malformed id `NOT_FOUND` or `VALIDATION`.
#[tokio::test]
async fn delete_keybind_removes_the_owners_keybind() {
    let f = setup().await;
    let keybind = f.create(&f.ws.bob, json!({})).await;
    for who in [&f.ws.carol, &f.ws.alice, &f.ws.olive] {
        assert_eq!(f.delete_code(who, id(&keybind)).await, "FORBIDDEN");
    }
    assert!(f.resolves(&f.ws.bob, id(&keybind)).await);

    let vars = Fixture::delete_vars(id(&keybind));
    let data = exec_ok(&f.schema, f.ws.bob.request(DELETE_KEYBIND, vars)).await;
    assert_eq!(data["deleted"], json!(true));
    assert!(!f.resolves(&f.ws.bob, id(&keybind)).await);
    assert_eq!(f.delete_code(&f.ws.bob, id(&keybind)).await, "NOT_FOUND");
    assert_eq!(f.delete_code(&f.ws.bob, "nope").await, "VALIDATION");
}

/// A workspace keybind runs as each member who presses it, so it may bind only a command that
/// changes nothing and preset only its own workspace's `server_id`; a user keybind may do both.
#[tokio::test]
async fn workspace_keybinds_bind_only_commands_that_change_nothing() {
    let f = setup().await;
    let mutating = json!({ "commandId": "chat.leave_conversation" });
    let foreign = json!({ "presetArgs": { "server_id": unknown_id() } });
    for extra in [mutating.clone(), foreign.clone()] {
        assert_eq!(
            f.create_code(&f.ws.alice, merged(f.workspace(), extra))
                .await,
            "VALIDATION"
        );
    }

    let own = json!({ "presetArgs": { "server_id": f.ws.server_id } });
    let shared = f.create(&f.ws.alice, merged(f.workspace(), own)).await;
    assert!(f.resolves(&f.ws.bob, id(&shared)).await);
    assert_eq!(
        f.update_code(&f.ws.alice, id(&shared), mutating.clone())
            .await,
        "VALIDATION"
    );

    let personal = f.create(&f.ws.bob, merged(mutating, foreign)).await;
    assert_eq!(personal["commandId"], json!("chat.leave_conversation"));
}
