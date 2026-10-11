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
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::WorkspaceEvent;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
use support::{RecordingBroadcaster, TestCtx};

const ENTRY_FIELDS: &str =
    "id serverId key value scope userId version expiresAt createdAt updatedAt";

fn set_mutation() -> String {
    format!(
        "mutation Set($input: SetWorkspaceStateInput!) {{ entry: setWorkspaceState(input: $input) {{ {ENTRY_FIELDS} }} }}"
    )
}

fn get_query() -> String {
    format!(
        "query Get($serverId: ID!, $key: String!, $scope: WorkspaceStateScopeGql!) {{ entry: workspaceState(serverId: $serverId, key: $key, scope: $scope) {{ {ENTRY_FIELDS} }} }}"
    )
}

const LIST_QUERY: &str = r#"
query List($serverId: ID!, $prefix: String!, $scope: WorkspaceStateScopeGql!, $limit: Int) {
  entries: workspaceStateList(serverId: $serverId, prefix: $prefix, scope: $scope, limit: $limit) {
    key value scope userId
  }
}
"#;

const DELETE_MUTATION: &str = r#"
mutation Delete($input: DeleteWorkspaceStateInput!) { deleted: deleteWorkspaceState(input: $input) }
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    events: std::sync::Arc<RecordingBroadcaster>,
    server_id: String,
    /// Owner.
    alice: Caller,
    /// Member.
    bob: Caller,
    /// Guest.
    gina: Caller,
    /// Not a member of the workspace.
    olive: Caller,
}

/// alice (Owner), bob (Member), gina (Guest) in one workspace and olive outside it.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("state-e2e").await;
    let alice = ctx.create_user("state-alice").await;
    let bob = ctx.create_user("state-bob").await;
    let gina = ctx.create_user("state-gina").await;
    let olive = ctx.create_user("state-olive").await;
    ctx.add_owner(&server, &alice).await;
    ctx.add_member(&server, &bob, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina, ServerMemberRole::Guest)
        .await;

    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    Fixture {
        alice: ctx.caller(&alice).await,
        bob: ctx.caller(&bob).await,
        gina: ctx.caller(&gina).await,
        olive: ctx.caller(&olive).await,
        server_id: server.id.to_string(),
        schema,
        events,
        ctx,
    }
}

impl Fixture {
    fn set_vars(&self, key: &str, value: Value, scope: &str, ttl: Option<i64>) -> Value {
        json!({ "input": {
            "serverId": self.server_id,
            "key": key,
            "value": value,
            "scope": scope,
            "ttlSeconds": ttl,
        } })
    }

    async fn set(&self, who: &Caller, key: &str, value: Value, scope: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(&set_mutation(), self.set_vars(key, value, scope, None)),
        )
        .await;
        data["entry"].clone()
    }

    async fn set_err(&self, who: &Caller, key: &str, value: Value, ttl: Option<i64>) -> String {
        exec_err_code(
            &self.schema,
            who.request(&set_mutation(), self.set_vars(key, value, "SERVER", ttl)),
        )
        .await
    }

    async fn get(&self, who: &Caller, key: &str, scope: &str) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(
                &get_query(),
                json!({ "serverId": self.server_id, "key": key, "scope": scope }),
            ),
        )
        .await;
        data["entry"].clone()
    }

    async fn list(
        &self,
        who: &Caller,
        prefix: &str,
        scope: &str,
        limit: Option<u64>,
    ) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(
                LIST_QUERY,
                json!({
                    "serverId": self.server_id,
                    "prefix": prefix,
                    "scope": scope,
                    "limit": limit,
                }),
            ),
        )
        .await;
        data["entries"].as_array().expect("entries").clone()
    }

    async fn delete(&self, who: &Caller, key: &str, scope: &str) -> bool {
        let data = exec_ok(
            &self.schema,
            who.request(
                DELETE_MUTATION,
                json!({ "input": { "serverId": self.server_id, "key": key, "scope": scope } }),
            ),
        )
        .await;
        data["deleted"].as_bool().expect("deleted")
    }

    /// Insert `count` entries named `seed.00001`, `seed.00002`, … straight into the table,
    /// owned by `owner` in the user scope or by nobody in the workspace scope.
    async fn seed(&self, count: u32, user_scope_owner: Option<&Caller>) {
        let (scope, user_id) = match user_scope_owner {
            Some(owner) => ("user", format!("'{}'::uuid", owner.user_id)),
            None => ("server", "NULL".to_string()),
        };
        let creator = self.alice.user_id;
        let sql = format!(
            "INSERT INTO workspace_state \
               (id, server_id, key, value, scope, user_id, created_by, updated_by, version, \
                expires_at, created_at, updated_at) \
             SELECT gen_random_uuid(), '{server}'::uuid, 'seed.' || lpad(i::text, 5, '0'), \
                    '1'::jsonb, '{scope}', {user_id}, '{creator}'::uuid, '{creator}'::uuid, 1, \
                    NULL, now(), now() \
             FROM generate_series(1, {count}) AS i",
            server = self.server_id,
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
            self.server_id
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
    let requests = [
        anonymous_request(
            &get_query(),
            json!({ "serverId": f.server_id, "key": "board.title", "scope": "SERVER" }),
        ),
        anonymous_request(
            LIST_QUERY,
            json!({ "serverId": f.server_id, "prefix": "board.", "scope": "SERVER" }),
        ),
        anonymous_request(
            &set_mutation(),
            f.set_vars("board.title", json!("x"), "SERVER", None),
        ),
        anonymous_request(
            DELETE_MUTATION,
            json!({ "input": { "serverId": f.server_id, "key": "board.title", "scope": "SERVER" } }),
        ),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "UNAUTHENTICATED");
    }
}

#[tokio::test]
async fn every_workspace_state_root_refuses_a_non_member() {
    let f = setup().await;
    f.set(&f.alice, "board.title", json!("x"), "SERVER").await;
    let requests = [
        f.olive.request(
            &get_query(),
            json!({ "serverId": f.server_id, "key": "board.title", "scope": "SERVER" }),
        ),
        f.olive.request(
            LIST_QUERY,
            json!({ "serverId": f.server_id, "prefix": "board.", "scope": "SERVER" }),
        ),
        f.olive.request(
            &set_mutation(),
            f.set_vars("board.title", json!("y"), "SERVER", None),
        ),
        f.olive.request(
            DELETE_MUTATION,
            json!({ "input": { "serverId": f.server_id, "key": "board.title", "scope": "SERVER" } }),
        ),
    ];
    for request in requests {
        assert_eq!(exec_err_code(&f.schema, request).await, "FORBIDDEN");
    }
    assert_eq!(f.get(&f.alice, "board.title", "SERVER").await["value"], "x");
}

#[tokio::test]
async fn a_guest_reads_and_writes_workspace_state_like_any_member() {
    let f = setup().await;
    f.set(&f.gina, "board.guest", json!(1), "SERVER").await;
    assert_eq!(f.get(&f.alice, "board.guest", "SERVER").await["value"], 1);
    assert!(f.delete(&f.gina, "board.guest", "SERVER").await);
}

// ── Happy paths ───────────────────────────────────────────────────────────

#[tokio::test]
async fn set_workspace_state_returns_the_stored_server_scoped_entry() {
    let f = setup().await;
    let value = json!({ "title": "Sprint 12", "open": [1, 2] });
    let entry = f
        .set(&f.alice, "board.title", value.clone(), "SERVER")
        .await;

    assert!(entry["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(entry["serverId"], f.server_id.as_str());
    assert_eq!(entry["key"], "board.title");
    assert_eq!(entry["value"], value);
    assert_eq!(entry["scope"], "SERVER");
    assert_eq!(entry["userId"], Value::Null);
    assert_eq!(entry["version"], 1);
    assert_eq!(entry["expiresAt"], Value::Null);
    assert!(entry["createdAt"].is_string());
    assert!(entry["updatedAt"].is_string());

    let read = f.get(&f.bob, "board.title", "SERVER").await;
    assert_eq!(read, entry, "another member reads the same entry");
}

#[tokio::test]
async fn overwriting_a_key_keeps_its_id_and_bumps_the_version() {
    let f = setup().await;
    let first = f.set(&f.alice, "board.title", json!("a"), "SERVER").await;
    let second = f.set(&f.bob, "board.title", json!("b"), "SERVER").await;
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["version"], 2);
    assert_eq!(second["value"], "b");
    assert_eq!(second["createdAt"], first["createdAt"]);
}

#[tokio::test]
async fn user_scoped_entries_belong_to_their_writer_only() {
    let f = setup().await;
    let entry = f.set(&f.alice, "prefs.theme", json!("dark"), "USER").await;
    assert_eq!(entry["scope"], "USER");
    assert_eq!(entry["userId"], f.alice.user_id.to_string().as_str());

    assert_eq!(f.get(&f.bob, "prefs.theme", "USER").await, Value::Null);
    assert_eq!(f.get(&f.alice, "prefs.theme", "SERVER").await, Value::Null);
    assert!(f.list(&f.bob, "prefs.", "USER", None).await.is_empty());

    f.set(&f.bob, "prefs.theme", json!("light"), "USER").await;
    assert_eq!(
        f.get(&f.alice, "prefs.theme", "USER").await["value"],
        "dark"
    );
    assert_eq!(f.get(&f.bob, "prefs.theme", "USER").await["value"], "light");

    assert!(!f.delete(&f.bob, "prefs.nope", "USER").await);
    assert!(f.delete(&f.bob, "prefs.theme", "USER").await);
    assert_eq!(
        f.get(&f.alice, "prefs.theme", "USER").await["value"],
        "dark",
        "deleting your own user-scoped key leaves another member's alone"
    );
}

#[tokio::test]
async fn reading_a_missing_key_returns_null() {
    let f = setup().await;
    assert_eq!(
        f.get(&f.alice, "board.missing", "SERVER").await,
        Value::Null
    );
}

#[tokio::test]
async fn delete_workspace_state_reports_whether_an_entry_was_removed() {
    let f = setup().await;
    f.set(&f.alice, "board.title", json!("x"), "SERVER").await;
    assert!(f.delete(&f.bob, "board.title", "SERVER").await);
    assert_eq!(f.get(&f.alice, "board.title", "SERVER").await, Value::Null);
    assert!(!f.delete(&f.bob, "board.title", "SERVER").await);
}

#[tokio::test]
async fn set_and_delete_announce_the_change_but_a_missed_delete_does_not() {
    let f = setup().await;
    f.set(&f.alice, "board.title", json!("x"), "SERVER").await;
    f.delete(&f.alice, "board.title", "SERVER").await;
    f.delete(&f.alice, "board.title", "SERVER").await;

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
    f.set(&f.alice, "board.b", json!(2), "SERVER").await;
    f.set(&f.alice, "board.a", json!(1), "SERVER").await;
    f.set(&f.alice, "other.c", json!(3), "SERVER").await;
    f.set(&f.alice, "board.u", json!(4), "USER").await;

    let keys: Vec<String> = f
        .list(&f.bob, "board.", "SERVER", None)
        .await
        .iter()
        .map(|e| e["key"].as_str().expect("key").to_string())
        .collect();
    assert_eq!(keys, ["board.a", "board.b"]);

    let limited = f.list(&f.bob, "board.", "SERVER", Some(1)).await;
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0]["key"], "board.a");
}

#[tokio::test]
async fn workspace_state_list_returns_a_hundred_entries_by_default_and_at_most_five_hundred() {
    let f = setup().await;
    f.seed(501, None).await;
    assert_eq!(f.list(&f.alice, "seed.", "SERVER", None).await.len(), 100);
    assert_eq!(
        f.list(&f.alice, "seed.", "SERVER", Some(10_000))
            .await
            .len(),
        500
    );
}

#[tokio::test]
async fn workspace_state_list_refuses_a_prefix_over_512_characters() {
    let f = setup().await;
    let long = "a".repeat(513);
    let code = exec_err_code(
        &f.schema,
        f.alice.request(
            LIST_QUERY,
            json!({ "serverId": f.server_id, "prefix": long, "scope": "SERVER" }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(
        f.list(&f.alice, &"a".repeat(512), "SERVER", None)
            .await
            .is_empty()
    );
}

// ── Validation ────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_workspace_state_refuses_a_malformed_or_reserved_key() {
    let f = setup().await;
    for key in [
        "a",
        "Board.title",
        "1board",
        "board.",
        "board title",
        "system.flags",
    ] {
        assert_eq!(
            f.set_err(&f.alice, key, json!(1), None).await,
            "VALIDATION",
            "key `{key}`"
        );
    }
}

#[tokio::test]
async fn set_workspace_state_refuses_a_value_over_64_kib() {
    let f = setup().await;
    // A JSON string serialises with two quote bytes around its content.
    let at_limit = "x".repeat(64 * 1024 - 2);
    f.set(&f.alice, "board.big", json!(at_limit), "SERVER")
        .await;

    let over = "x".repeat(64 * 1024 - 1);
    assert_eq!(
        f.set_err(&f.alice, "board.big", json!(over), None).await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn set_workspace_state_accepts_a_ttl_from_one_second_to_thirty_days() {
    let f = setup().await;
    for ttl in [0, -5, 30 * 24 * 3600 + 1] {
        assert_eq!(
            f.set_err(&f.alice, "board.ttl", json!(1), Some(ttl)).await,
            "VALIDATION",
            "ttl {ttl}"
        );
    }

    let before = chrono::Utc::now();
    let data = exec_ok(
        &f.schema,
        f.alice.request(
            &set_mutation(),
            f.set_vars("board.ttl", json!(1), "SERVER", Some(30 * 24 * 3600)),
        ),
    )
    .await;
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
    f.set(&f.alice, "board.temp", json!(1), "SERVER").await;
    f.set(&f.alice, "board.keep", json!(2), "SERVER").await;
    f.expire("board.temp").await;

    assert_eq!(f.get(&f.alice, "board.temp", "SERVER").await, Value::Null);
    let keys: Vec<Value> = f
        .list(&f.alice, "board.", "SERVER", None)
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
    f.seed(1000, Some(&f.bob)).await;

    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            &set_mutation(),
            f.set_vars("prefs.extra", json!(1), "USER", None),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");

    let overwritten = f.set(&f.bob, "seed.00001", json!(2), "USER").await;
    assert_eq!(
        overwritten["version"], 2,
        "overwriting an existing key is allowed"
    );

    f.set(&f.alice, "prefs.extra", json!(1), "USER").await;
    f.set(&f.bob, "board.extra", json!(1), "SERVER").await;
}

#[tokio::test]
async fn a_workspace_holds_at_most_ten_thousand_server_scoped_entries() {
    let f = setup().await;
    f.seed(10_000, None).await;
    assert_eq!(
        f.set_err(&f.alice, "board.extra", json!(1), None).await,
        "VALIDATION"
    );

    f.expire("seed.00001").await;
    f.set(&f.alice, "board.extra", json!(1), "SERVER").await;
}
