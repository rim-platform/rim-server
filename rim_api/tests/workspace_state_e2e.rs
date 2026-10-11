//! Through-the-stack e2e tests for the workspace state roots: `workspaceState`,
//! `workspaceStateList`, `setWorkspaceState` and `deleteWorkspaceState`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::WorkspaceEvent;

use support::graphql::{
    Call, Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code,
    exec_ok,
};
use support::roles::Roles;
use support::{RecordingBroadcaster, TestCtx};

macro_rules! entry_fields {
    () => {
        "id serverId key value scope userId version expiresAt createdAt updatedAt"
    };
}

const SET_STATE: &str = concat!(
    "mutation Set($input: SetWorkspaceStateInput!) { entry: setWorkspaceState(input: $input) { ",
    entry_fields!(),
    " } }"
);

const GET_STATE: &str = concat!(
    "query Get($serverId: ID!, $key: String!, $scope: WorkspaceStateScopeGql!) { ",
    "entry: workspaceState(serverId: $serverId, key: $key, scope: $scope) { ",
    entry_fields!(),
    " } }"
);

const LIST_STATE: &str = r#"
query List($serverId: ID!, $prefix: String!, $scope: WorkspaceStateScopeGql!, $limit: Int) {
  entries: workspaceStateList(serverId: $serverId, prefix: $prefix, scope: $scope, limit: $limit) {
    key value scope userId
  }
}
"#;

const DELETE_STATE: &str = r#"
mutation Delete($input: DeleteWorkspaceStateInput!) { deleted: deleteWorkspaceState(input: $input) }
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    events: std::sync::Arc<RecordingBroadcaster>,
    ws: Roles,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("state").await;
    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    Fixture {
        schema,
        events,
        ws,
        ctx,
    }
}

impl Fixture {
    fn set_call(&self, key: &str, value: Value, scope: &str, ttl: Option<i64>) -> Call {
        (
            SET_STATE,
            json!({ "input": {
                "serverId": self.ws.server_id,
                "key": key,
                "value": value,
                "scope": scope,
                "ttlSeconds": ttl,
            } }),
        )
    }

    fn get_call(&self, key: &str, scope: &str) -> Call {
        (
            GET_STATE,
            json!({ "serverId": self.ws.server_id, "key": key, "scope": scope }),
        )
    }

    fn list_call(&self, prefix: &str, scope: &str, limit: Option<u64>) -> Call {
        (
            LIST_STATE,
            json!({
                "serverId": self.ws.server_id,
                "prefix": prefix,
                "scope": scope,
                "limit": limit,
            }),
        )
    }

    fn delete_call(&self, key: &str, scope: &str) -> Call {
        (
            DELETE_STATE,
            json!({ "input": { "serverId": self.ws.server_id, "key": key, "scope": scope } }),
        )
    }

    /// One call to every workspace state root, all on the key `board.title`.
    fn every_root(&self) -> Vec<Call> {
        vec![
            self.get_call("board.title", "SERVER"),
            self.list_call("board.", "SERVER", None),
            self.set_call("board.title", json!("y"), "SERVER", None),
            self.delete_call("board.title", "SERVER"),
        ]
    }

    async fn code(&self, who: &Caller, call: Call) -> String {
        exec_err_code(&self.schema, who.call(call)).await
    }

    /// The code `who` gets for writing `value` to the workspace-scoped `key` with `ttl`.
    async fn set_code(&self, who: &Caller, key: &str, value: Value, ttl: Option<i64>) -> String {
        self.code(who, self.set_call(key, value, "SERVER", ttl))
            .await
    }

    async fn set(&self, who: &Caller, key: &str, value: Value, scope: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.call(self.set_call(key, value, scope, None)),
        )
        .await;
        data["entry"].clone()
    }

    async fn get(&self, who: &Caller, key: &str, scope: &str) -> Value {
        let data = exec_ok(&self.schema, who.call(self.get_call(key, scope))).await;
        data["entry"].clone()
    }

    async fn list(
        &self,
        who: &Caller,
        prefix: &str,
        scope: &str,
        limit: Option<u64>,
    ) -> Vec<Value> {
        let call = self.list_call(prefix, scope, limit);
        let data = exec_ok(&self.schema, who.call(call)).await;
        data["entries"].as_array().expect("entries").clone()
    }

    async fn delete(&self, who: &Caller, key: &str, scope: &str) -> bool {
        let data = exec_ok(&self.schema, who.call(self.delete_call(key, scope))).await;
        data["deleted"].as_bool().expect("deleted")
    }

    /// Insert `count` entries named `seed.00001`, `seed.00002`, … straight into the table,
    /// owned by `owner` in the user scope or by nobody in the workspace scope.
    async fn seed(&self, count: u32, user_scope_owner: Option<&Caller>) {
        let (scope, user_id) = match user_scope_owner {
            Some(owner) => ("user", format!("'{}'::uuid", owner.user_id)),
            None => ("server", "NULL".to_string()),
        };
        let creator = self.ws.alice.user_id;
        let sql = format!(
            "INSERT INTO workspace_state \
               (id, server_id, key, value, scope, user_id, created_by, updated_by, version, \
                expires_at, created_at, updated_at) \
             SELECT gen_random_uuid(), '{server}'::uuid, 'seed.' || lpad(i::text, 5, '0'), \
                    '1'::jsonb, '{scope}', {user_id}, '{creator}'::uuid, '{creator}'::uuid, 1, \
                    NULL, now(), now() \
             FROM generate_series(1, {count}) AS i",
            server = self.ws.server_id,
        );
        self.ctx
            .conn
            .execute_unprepared(&sql)
            .await
            .expect("seed workspace state");
    }

    /// Move the expiry of `key` into the past.
    async fn expire(&self, key: &str) {
        let sql = format!(
            "UPDATE workspace_state SET expires_at = now() - interval '1 second' \
             WHERE server_id = '{}'::uuid AND key = '{key}'",
            self.ws.server_id
        );
        self.ctx
            .conn
            .execute_unprepared(&sql)
            .await
            .expect("expire entry");
    }
}

// ── Authentication and membership ─────────────────────────────────────────

#[tokio::test]
async fn every_workspace_state_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    assert_each_refused(
        &f.schema,
        &f.every_root(),
        "UNAUTHENTICATED",
        anonymous_request,
    )
    .await;
}

#[tokio::test]
async fn every_workspace_state_root_refuses_a_non_member() {
    let f = setup().await;
    f.set(&f.ws.alice, "board.title", json!("x"), "SERVER")
        .await;
    assert_each_refused_as(&f.schema, &f.ws.olive, &f.every_root(), "FORBIDDEN").await;
    assert_eq!(
        f.get(&f.ws.alice, "board.title", "SERVER").await["value"],
        "x"
    );
}

#[tokio::test]
async fn a_guest_reads_and_writes_workspace_state_like_any_member() {
    let f = setup().await;
    f.set(&f.ws.gina, "board.guest", json!(1), "SERVER").await;
    assert_eq!(
        f.get(&f.ws.alice, "board.guest", "SERVER").await["value"],
        1
    );
    assert!(f.delete(&f.ws.gina, "board.guest", "SERVER").await);
}

// ── Happy paths ───────────────────────────────────────────────────────────

#[tokio::test]
async fn set_workspace_state_returns_the_stored_server_scoped_entry() {
    let f = setup().await;
    let value = json!({ "title": "Sprint 12", "open": [1, 2] });
    let entry = f
        .set(&f.ws.alice, "board.title", value.clone(), "SERVER")
        .await;

    assert!(entry["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(entry["serverId"], f.ws.server_id.as_str());
    assert_eq!(entry["key"], "board.title");
    assert_eq!(entry["value"], value);
    assert_eq!(entry["scope"], "SERVER");
    assert_eq!(entry["userId"], Value::Null);
    assert_eq!(entry["version"], 1);
    assert_eq!(entry["expiresAt"], Value::Null);
    assert!(entry["createdAt"].is_string());
    assert!(entry["updatedAt"].is_string());

    let read = f.get(&f.ws.bob, "board.title", "SERVER").await;
    assert_eq!(read, entry, "another member reads the same entry");
}

#[tokio::test]
async fn overwriting_a_key_keeps_its_id_and_bumps_the_version() {
    let f = setup().await;
    let first = f
        .set(&f.ws.alice, "board.title", json!("a"), "SERVER")
        .await;
    let second = f.set(&f.ws.bob, "board.title", json!("b"), "SERVER").await;
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["version"], 2);
    assert_eq!(second["value"], "b");
    assert_eq!(second["createdAt"], first["createdAt"]);
}

#[tokio::test]
async fn user_scoped_entries_belong_to_their_writer_only() {
    let f = setup().await;
    let entry = f
        .set(&f.ws.alice, "prefs.theme", json!("dark"), "USER")
        .await;
    assert_eq!(entry["scope"], "USER");
    assert_eq!(entry["userId"], f.ws.alice.user_id.to_string().as_str());

    assert_eq!(f.get(&f.ws.bob, "prefs.theme", "USER").await, Value::Null);
    assert_eq!(
        f.get(&f.ws.alice, "prefs.theme", "SERVER").await,
        Value::Null
    );
    assert!(f.list(&f.ws.bob, "prefs.", "USER", None).await.is_empty());

    f.set(&f.ws.bob, "prefs.theme", json!("light"), "USER")
        .await;
    assert_eq!(
        f.get(&f.ws.alice, "prefs.theme", "USER").await["value"],
        "dark"
    );
    assert_eq!(
        f.get(&f.ws.bob, "prefs.theme", "USER").await["value"],
        "light"
    );

    assert!(!f.delete(&f.ws.bob, "prefs.nope", "USER").await);
    assert!(f.delete(&f.ws.bob, "prefs.theme", "USER").await);
    assert_eq!(
        f.get(&f.ws.alice, "prefs.theme", "USER").await["value"],
        "dark",
        "deleting your own user-scoped key leaves another member's alone"
    );
}

#[tokio::test]
async fn reading_a_missing_key_returns_null() {
    let f = setup().await;
    assert_eq!(
        f.get(&f.ws.alice, "board.missing", "SERVER").await,
        Value::Null
    );
}

#[tokio::test]
async fn delete_workspace_state_reports_whether_an_entry_was_removed() {
    let f = setup().await;
    f.set(&f.ws.alice, "board.title", json!("x"), "SERVER")
        .await;
    assert!(f.delete(&f.ws.bob, "board.title", "SERVER").await);
    assert_eq!(
        f.get(&f.ws.alice, "board.title", "SERVER").await,
        Value::Null
    );
    assert!(!f.delete(&f.ws.bob, "board.title", "SERVER").await);
}

#[tokio::test]
async fn set_and_delete_announce_the_change_but_a_missed_delete_does_not() {
    let f = setup().await;
    f.set(&f.ws.alice, "board.title", json!("x"), "SERVER")
        .await;
    f.delete(&f.ws.alice, "board.title", "SERVER").await;
    f.delete(&f.ws.alice, "board.title", "SERVER").await;

    let state_events: Vec<WorkspaceEvent> = f
        .events
        .events()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                WorkspaceEvent::WorkspaceStateChanged { .. }
                    | WorkspaceEvent::WorkspaceStateDeleted { .. }
            )
        })
        .collect();
    assert_eq!(state_events.len(), 2, "events: {state_events:?}");
    assert!(matches!(
        &state_events[0],
        WorkspaceEvent::WorkspaceStateChanged { key, version: 1, .. } if key == "board.title"
    ));
    assert!(matches!(
        &state_events[1],
        WorkspaceEvent::WorkspaceStateDeleted { key, .. } if key == "board.title"
    ));
}

// ── Listing ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn workspace_state_list_returns_keys_under_the_prefix_in_key_order() {
    let f = setup().await;
    f.set(&f.ws.alice, "board.b", json!(2), "SERVER").await;
    f.set(&f.ws.alice, "board.a", json!(1), "SERVER").await;
    f.set(&f.ws.alice, "other.c", json!(3), "SERVER").await;
    f.set(&f.ws.alice, "board.u", json!(4), "USER").await;

    let keys: Vec<String> = f
        .list(&f.ws.bob, "board.", "SERVER", None)
        .await
        .iter()
        .map(|e| e["key"].as_str().expect("key").to_string())
        .collect();
    assert_eq!(keys, ["board.a", "board.b"]);

    let limited = f.list(&f.ws.bob, "board.", "SERVER", Some(1)).await;
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0]["key"], "board.a");
}

#[tokio::test]
async fn workspace_state_list_returns_a_hundred_entries_by_default_and_at_most_five_hundred() {
    let f = setup().await;
    f.seed(501, None).await;
    assert_eq!(
        f.list(&f.ws.alice, "seed.", "SERVER", None).await.len(),
        100
    );
    assert_eq!(
        f.list(&f.ws.alice, "seed.", "SERVER", Some(10_000))
            .await
            .len(),
        500
    );
}

#[tokio::test]
async fn workspace_state_list_refuses_a_prefix_over_512_characters() {
    let f = setup().await;
    let call = f.list_call(&"a".repeat(513), "SERVER", None);
    assert_eq!(f.code(&f.ws.alice, call).await, "VALIDATION");
    assert!(
        f.list(&f.ws.alice, &"a".repeat(512), "SERVER", None)
            .await
            .is_empty()
    );
}

// ── Validation ────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_workspace_state_refuses_a_malformed_or_reserved_key() {
    let f = setup().await;
    let calls = [
        "a",
        "Board.title",
        "1board",
        "board.",
        "board title",
        "system.flags",
    ]
    .map(|key| f.set_call(key, json!(1), "SERVER", None));
    assert_each_refused_as(&f.schema, &f.ws.alice, &calls, "VALIDATION").await;
}

#[tokio::test]
async fn set_workspace_state_refuses_a_value_over_64_kib() {
    let f = setup().await;
    // A JSON string serialises with two quote bytes around its content.
    let at_limit = "x".repeat(64 * 1024 - 2);
    f.set(&f.ws.alice, "board.big", json!(at_limit), "SERVER")
        .await;

    let over = "x".repeat(64 * 1024 - 1);
    assert_eq!(
        f.set_code(&f.ws.alice, "board.big", json!(over), None)
            .await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn set_workspace_state_accepts_a_ttl_from_one_second_to_thirty_days() {
    let f = setup().await;
    let calls = [0, -5, 30 * 24 * 3600 + 1]
        .map(|ttl| f.set_call("board.ttl", json!(1), "SERVER", Some(ttl)));
    assert_each_refused_as(&f.schema, &f.ws.alice, &calls, "VALIDATION").await;

    let before = chrono::Utc::now();
    let call = f.set_call("board.ttl", json!(1), "SERVER", Some(30 * 24 * 3600));
    let data = exec_ok(&f.schema, f.ws.alice.call(call)).await;
    let expires_at: chrono::DateTime<chrono::Utc> = data["entry"]["expiresAt"]
        .as_str()
        .expect("expiresAt")
        .parse()
        .expect("RFC 3339");
    let expected = before + chrono::Duration::days(30);
    assert!(
        (expires_at - expected).num_seconds().abs() < 60,
        "expiresAt {expires_at} is thirty days out"
    );
}

#[tokio::test]
async fn an_expired_entry_is_neither_read_nor_listed() {
    let f = setup().await;
    f.set(&f.ws.alice, "board.temp", json!(1), "SERVER").await;
    f.set(&f.ws.alice, "board.keep", json!(2), "SERVER").await;
    f.expire("board.temp").await;

    assert_eq!(
        f.get(&f.ws.alice, "board.temp", "SERVER").await,
        Value::Null
    );
    let keys: Vec<Value> = f
        .list(&f.ws.alice, "board.", "SERVER", None)
        .await
        .iter()
        .map(|e| e["key"].clone())
        .collect();
    assert_eq!(keys, [json!("board.keep")]);
}

// ── Quotas ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_member_holds_at_most_a_thousand_user_scoped_entries() {
    let f = setup().await;
    f.seed(1000, Some(&f.ws.bob)).await;

    let call = f.set_call("prefs.extra", json!(1), "USER", None);
    assert_eq!(f.code(&f.ws.bob, call).await, "VALIDATION");

    let overwritten = f.set(&f.ws.bob, "seed.00001", json!(2), "USER").await;
    assert_eq!(
        overwritten["version"], 2,
        "overwriting an existing key is allowed"
    );

    f.set(&f.ws.alice, "prefs.extra", json!(1), "USER").await;
    f.set(&f.ws.bob, "board.extra", json!(1), "SERVER").await;
}

#[tokio::test]
async fn a_workspace_holds_at_most_ten_thousand_server_scoped_entries() {
    let f = setup().await;
    f.seed(10_000, None).await;
    assert_eq!(
        f.set_code(&f.ws.alice, "board.extra", json!(1), None).await,
        "VALIDATION"
    );

    f.expire("seed.00001").await;
    f.set(&f.ws.alice, "board.extra", json!(1), "SERVER").await;
}
