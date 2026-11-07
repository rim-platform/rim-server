//! The Home layout over a real database: reading, saving with the newest edit kept, resetting,
//! the checks, who is refused, and the layout going with the account at the purge.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test home_layout_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_infra_services::scheduled_purges::purge_deleted_accounts;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err, exec_ok,
};

const READ: &str = "query { homeLayout { layout editedAt updatedAt } }";

const SAVE: &str = r#"mutation($layout: JSON!, $editedAt: DateTime!) {
  saveHomeLayout(layout: $layout, editedAt: $editedAt) {
    kept layout { layout editedAt updatedAt }
  }
}"#;

const RESET: &str = "mutation { resetHomeLayout }";

const REQUEST_DELETION: &str = r#"mutation($u: String!) {
  requestAccountDeletion(input: { confirmUsername: $u }) { scheduledFor }
}"#;

fn layout(id: &str) -> Value {
    json!({ "version": 1, "boxes": [
        { "id": id, "kind": "waiting", "width": "half" },
        { "id": "c", "kind": "channel", "width": "full",
          "settings": { "workspaceId": "w", "conversationId": "c", "count": 5 } }
    ]})
}

fn save(who: &Caller, layout: Value, edited_at: DateTime<Utc>) -> async_graphql::Request {
    who.request(
        SAVE,
        json!({ "layout": layout, "editedAt": edited_at.to_rfc3339() }),
    )
}

fn time(value: &Value) -> DateTime<Utc> {
    value.as_str().expect("a time").parse().expect("RFC 3339")
}

async fn set_kind(ctx: &TestCtx, who: &Caller, kind: &str) {
    ctx.conn
        .execute_raw(Statement::from_string(
            DbBackend::Postgres,
            format!(
                r#"UPDATE "user" SET kind = '{kind}' WHERE id = '{}'"#,
                who.user_id
            ),
        ))
        .await
        .expect("kind");
}

async fn rows(ctx: &TestCtx, who: &Caller) -> i64 {
    ctx.conn
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!(
                "SELECT count(*) FROM home_layout WHERE user_id = '{}'",
                who.user_id
            ),
        ))
        .await
        .expect("count")
        .expect("a row")
        .try_get_by_index::<i64>(0)
        .expect("count")
}

/// Nothing is stored at first; a save is stored and read back; an older edit answers the stored
/// layout with `kept: false`; an edit at the same time or later replaces it; an `editedAt` in the
/// future is stored as the server's clock; a reset deletes it.
#[tokio::test]
async fn the_newest_edit_is_kept_and_a_reset_goes_back_to_none() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = ctx.create_user("ada").await;
    let ada = ctx.caller(&ada).await;

    assert_eq!(
        exec_ok(&schema, ada.request(READ, json!({}))).await["homeLayout"],
        Value::Null
    );

    let made = Utc::now() - Duration::minutes(10);
    let first = exec_ok(&schema, save(&ada, layout("first"), made)).await;
    assert_eq!(first["saveHomeLayout"]["kept"], json!(true));
    assert_eq!(first["saveHomeLayout"]["layout"]["layout"], layout("first"));
    let stored_at = time(&first["saveHomeLayout"]["layout"]["editedAt"]);
    assert!(
        (stored_at - made).num_milliseconds().abs() < 1,
        "{stored_at}"
    );

    let read = exec_ok(&schema, ada.request(READ, json!({}))).await;
    assert_eq!(read["homeLayout"]["layout"], layout("first"));

    let older = exec_ok(
        &schema,
        save(&ada, layout("older"), made - Duration::minutes(1)),
    )
    .await;
    assert_eq!(older["saveHomeLayout"]["kept"], json!(false));
    assert_eq!(older["saveHomeLayout"]["layout"]["layout"], layout("first"));

    let same = exec_ok(&schema, save(&ada, layout("same"), stored_at)).await;
    assert_eq!(same["saveHomeLayout"]["kept"], json!(true));
    assert_eq!(same["saveHomeLayout"]["layout"]["layout"], layout("same"));

    let future = exec_ok(
        &schema,
        save(&ada, layout("future"), Utc::now() + Duration::days(30)),
    )
    .await;
    assert_eq!(future["saveHomeLayout"]["kept"], json!(true));
    let clamped = time(&future["saveHomeLayout"]["layout"]["editedAt"]);
    assert!(clamped <= Utc::now(), "{clamped}");

    let now = exec_ok(&schema, save(&ada, layout("now"), Utc::now())).await;
    assert_eq!(
        now["saveHomeLayout"]["kept"],
        json!(true),
        "a clamped edit doesn't win over the next real one"
    );

    assert_eq!(
        exec_ok(&schema, ada.request(RESET, json!({}))).await["resetHomeLayout"],
        json!(true)
    );
    assert_eq!(
        exec_ok(&schema, ada.request(READ, json!({}))).await["homeLayout"],
        Value::Null
    );
    assert_eq!(
        exec_ok(&schema, ada.request(RESET, json!({}))).await["resetHomeLayout"],
        json!(false)
    );
    let after_reset = exec_ok(&schema, save(&ada, layout("old"), made - Duration::days(1))).await;
    assert_eq!(
        after_reset["saveHomeLayout"]["kept"],
        json!(true),
        "with nothing stored any edit is kept"
    );
}

/// A refused layout stores nothing and the message names the check.
#[tokio::test]
async fn a_refused_layout_names_the_check_and_stores_nothing() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = ctx.create_user("ada").await;
    let ada = ctx.caller(&ada).await;
    let boxes: Vec<Value> = (0..31)
        .map(|i| json!({ "id": format!("b{i}"), "kind": "waiting", "width": "half" }))
        .collect();
    for (bad, message) in [
        (
            json!({ "version": 1, "boxes": boxes }),
            "A Home layout holds at most 30 boxes",
        ),
        (
            json!({ "version": 1, "boxes": [{ "id": "a", "kind": "waiting", "width": "wide" }] }),
            "A box width must be half or full",
        ),
        (
            json!({ "version": 1, "boxes": [{ "id": "x".repeat(65), "kind": "k", "width": "half" }] }),
            "A box id must be 1 to 64 characters",
        ),
        (
            json!({ "version": 2, "boxes": [] }),
            "A Home layout's version must be 1",
        ),
        (
            json!({ "version": 1, "boxes": [{ "id": "a", "kind": "k", "width": "half",
                    "settings": { "note": "x".repeat(16 * 1024) } }] }),
            "A Home layout must be at most 16 KiB",
        ),
    ] {
        let (code, got) = exec_err(&schema, save(&ada, bad, Utc::now())).await;
        assert_eq!(code, "VALIDATION");
        assert!(got.contains(message), "{got}");
    }
    assert_eq!(rows(&ctx, &ada).await, 0);
}

/// Each account reads and resets only its own layout.
#[tokio::test]
async fn nobody_reads_or_changes_another_persons_layout() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;
    let (ada, bob) = (ctx.caller(&ada).await, ctx.caller(&bob).await);
    exec_ok(&schema, save(&ada, layout("ada"), Utc::now())).await;

    assert_eq!(
        exec_ok(&schema, bob.request(READ, json!({}))).await["homeLayout"],
        Value::Null
    );
    assert_eq!(
        exec_ok(&schema, bob.request(RESET, json!({}))).await["resetHomeLayout"],
        json!(false)
    );
    exec_ok(
        &schema,
        save(&bob, layout("bob"), Utc::now() + Duration::days(1)),
    )
    .await;
    assert_eq!(
        exec_ok(&schema, ada.request(READ, json!({}))).await["homeLayout"]["layout"],
        layout("ada")
    );
}

/// No session is `UNAUTHENTICATED`; a person from another server and a bot are `FORBIDDEN`
/// from every root.
#[tokio::test]
async fn people_from_other_servers_bots_and_strangers_are_refused() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let calls = [
        (READ, json!({})),
        (
            SAVE,
            json!({ "layout": layout("x"), "editedAt": Utc::now().to_rfc3339() }),
        ),
        (RESET, json!({})),
    ];
    assert_each_refused(&schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
    for kind in ["remote", "bot"] {
        let user = ctx.create_user(kind).await;
        let who = ctx.caller(&user).await;
        set_kind(&ctx, &who, kind).await;
        assert_each_refused_as(&schema, &who, &calls, "FORBIDDEN").await;
        assert_eq!(rows(&ctx, &who).await, 0, "{kind}");
    }
}

/// The layout stays while the deletion waits out its grace period and the purge removes it.
#[tokio::test]
async fn the_layout_stays_through_the_grace_period_and_goes_with_the_purge() {
    let ctx = TestCtx::new().await;
    let (schema, provider) =
        ctx.build_with_deletion_grace(Duration::days(14), RegistrationMode::Open);
    let ada = ctx.enrolled("ada", 61).await;
    let bob = ctx.enrolled("bob", 62).await;
    exec_ok(&schema, save(&ada.caller, layout("ada"), Utc::now())).await;
    exec_ok(&schema, save(&bob.caller, layout("bob"), Utc::now())).await;
    exec_ok(
        &schema,
        ada.caller.request(REQUEST_DELETION, json!({ "u": "ada" })),
    )
    .await;

    let waiting = purge_deleted_accounts(provider.account_deletion_service().as_ref())
        .await
        .expect("run");
    assert_eq!(waiting.purged.len(), 0);
    assert_eq!(rows(&ctx, &ada.caller).await, 1);
    assert_eq!(
        exec_ok(&schema, ada.caller.request(READ, json!({}))).await["homeLayout"]["layout"],
        layout("ada")
    );

    ctx.conn
        .execute_unprepared(&format!(
            r#"UPDATE "user" SET deletion_scheduled_for = now() - interval '1 minute'
               WHERE id = '{}'"#,
            ada.caller.user_id
        ))
        .await
        .expect("grace over");
    let run = purge_deleted_accounts(provider.account_deletion_service().as_ref())
        .await
        .expect("run");
    assert_eq!(run.failed, vec![]);
    assert_eq!(run.purged.len(), 1);
    assert_eq!(rows(&ctx, &ada.caller).await, 0);
    assert_eq!(rows(&ctx, &bob.caller).await, 1);
}
