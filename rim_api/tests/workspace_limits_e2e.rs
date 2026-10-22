//! The caps that keep one account from filling a workspace or the instance, over a real
//! database: workspaces per user under concurrent creation and on a hand-over, channels per
//! workspace with archived channels and the ceiling on all of them, channel creation in the default workspace, and the
//! macros and pipelines of one member.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test workspace_limits_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use rim_services::auth::dto::RegistrationMode;
use rim_services::limits::InstanceLimits;

use support::TestCtx;
use support::graphql::{Caller, exec_err, exec_err_code, exec_ok};

const CREATE_SERVER: &str =
    "mutation($name: String!) { createServer(input: { name: $name }) { id } }";
const TRANSFER: &str = r#"mutation($s: ID!, $u: ID!) {
  transferWorkspaceOwnership(input: { serverId: $s, userId: $u }) { id }
}"#;
const CREATE_CHANNEL: &str = "mutation($s: ID!, $title: String!, $private: Boolean) { \
     createConversation(input: { serverId: $s, isChannel: true, title: $title, \
     isPrivate: $private }) { id } }";
const SET_ARCHIVED: &str = "mutation($s: ID!, $c: ID!, $archived: Boolean!) { \
     setConversationArchived(serverId: $s, conversationId: $c, archived: $archived) { id } }";
const CREATE_MACRO: &str =
    "mutation($input: CreateMacroInput!) { createMacro(input: $input) { id } }";
const CREATE_PIPELINE: &str =
    "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }";
const KICK: &str = "mutation($s: ID!, $u: ID!) { kickMember(serverId: $s, userId: $u) }";
const BAN: &str = "mutation($s: ID!, $u: ID!) { banMember(serverId: $s, userId: $u) { userId } }";

fn limits(f: impl FnOnce(&mut InstanceLimits)) -> InstanceLimits {
    let mut limits = InstanceLimits::default();
    f(&mut limits);
    limits
}

/// The rows of `table` in the workspace `server_id` whose `column` is `user`.
async fn count_rows(
    ctx: &TestCtx,
    table: &str,
    column: &str,
    server_id: &str,
    user: &Caller,
) -> i64 {
    let row = ctx
        .conn
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            format!(
                "SELECT count(*) AS n FROM {table} WHERE server_id = $1::uuid AND {column} = $2::uuid"
            ),
            vec![server_id.into(), user.user_id.to_string().into()],
        ))
        .await
        .expect("count")
        .expect("row");
    row.try_get::<i64>("", "n").expect("n")
}

/// Two workspaces created at once by an account allowed one more: one is created, the other
/// refused, because the count and the creation run under a lock on the account.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creations_pass_the_workspaces_per_user_cap_once() {
    let ctx = TestCtx::new().await;
    let ada = ctx.create_user("cap-ada").await;
    let ada = ctx.caller(&ada).await;
    let (schema, _) = ctx.build_with_limits(limits(|l| l.workspaces_per_user = Some(1)));

    let create = |name: &str| schema.execute(ada.request(CREATE_SERVER, json!({ "name": name })));
    let (first, second) = tokio::join!(create("first"), create("second"));
    let created = [&first, &second]
        .iter()
        .filter(|r| r.errors.is_empty())
        .count();
    assert_eq!(created, 1, "{first:?} {second:?}");
    let refused = if first.errors.is_empty() {
        &second
    } else {
        &first
    };
    assert_eq!(
        refused.errors[0].message,
        "this instance allows 1 workspaces per user"
    );
}

/// A workspace is handed only to a member who owns fewer workspaces than the cap.
#[tokio::test]
async fn ownership_goes_only_to_a_member_below_the_workspaces_per_user_cap() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("cap-transfer").await;
    let (schema, _) = ctx.build_with_limits(limits(|l| l.workspaces_per_user = Some(1)));
    let to = |who: &Caller| json!({ "s": ws.server_id, "u": who.user_id.to_string() });

    exec_ok(
        &schema,
        ws.bob.request(CREATE_SERVER, json!({ "name": "bob's" })),
    )
    .await;
    let (code, message) = exec_err(&schema, ws.alice.request(TRANSFER, to(&ws.bob))).await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(
        message,
        "this instance allows 1 workspaces per user, and the new owner already owns 1"
    );

    exec_ok(&schema, ws.alice.request(TRANSFER, to(&ws.carol))).await;
}

/// Archived channels do not count against the channel cap; unarchiving one needs room.
#[tokio::test]
async fn archived_channels_leave_room_under_the_channels_per_workspace_cap() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("cap-channels").await;
    let (schema, _) = ctx.build_with_limits(limits(|l| l.channels_per_workspace = Some(1)));
    let create = |title: &str| {
        ws.alice.request(
            CREATE_CHANNEL,
            json!({ "s": ws.server_id, "title": title, "private": true }),
        )
    };
    let archive = |id: &Value, archived: bool| {
        ws.alice.request(
            SET_ARCHIVED,
            json!({ "s": ws.server_id, "c": id, "archived": archived }),
        )
    };

    let first = exec_ok(&schema, create("first")).await["createConversation"]["id"].clone();
    assert_eq!(exec_err_code(&schema, create("second")).await, "VALIDATION");

    exec_ok(&schema, archive(&first, true)).await;
    exec_ok(&schema, create("second")).await;
    let (code, message) = exec_err(&schema, archive(&first, false)).await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(message, "this instance allows 1 channels per workspace");
}

/// Creating and archiving channels stops at five times the channel cap, archived channels
/// included; the refusal says archived channels still count.
#[tokio::test]
async fn archiving_does_not_free_room_under_the_ceiling_on_all_channels() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("cap-all-channels").await;
    let (schema, _) = ctx.build_with_limits(limits(|l| l.channels_per_workspace = Some(1)));
    let create = |title: String| {
        ws.alice.request(
            CREATE_CHANNEL,
            json!({ "s": ws.server_id, "title": title, "private": false }),
        )
    };

    for n in 0..5 {
        let id = exec_ok(&schema, create(format!("round-{n}"))).await["createConversation"]["id"]
            .clone();
        exec_ok(
            &schema,
            ws.alice.request(
                SET_ARCHIVED,
                json!({ "s": ws.server_id, "c": id, "archived": true }),
            ),
        )
        .await;
    }
    let (code, message) = exec_err(&schema, create("round-5".to_string())).await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(
        message,
        "this instance allows 5 channels per workspace counting archived ones; archived \
         channels cannot be deleted and still count toward it"
    );

    // Another workspace has its own room.
    ctx.join(
        &ws.other_server_id,
        &ws.alice,
        rim_core::server_member::model::ServerMemberRole::Owner,
    )
    .await;
    exec_ok(
        &schema,
        ws.alice.request(
            CREATE_CHANNEL,
            json!({ "s": ws.other_server_id, "title": "round-5", "private": false }),
        ),
    )
    .await;
}

/// In the default workspace only the Owner and Moderators create channels, public or private;
/// in any other workspace a member still does.
#[tokio::test]
async fn in_the_default_workspace_only_owners_and_moderators_create_channels() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("default-channels").await;
    let default_id = ws
        .server_id
        .parse::<uuid::Uuid>()
        .expect("server id")
        .into();
    let schema = ctx.build_schema_with_default_workspace(default_id, RegistrationMode::InviteOnly);
    let create = |who: &Caller, s: &str, title: &str, private: bool| {
        who.request(
            CREATE_CHANNEL,
            json!({ "s": s, "title": title, "private": private }),
        )
    };

    for (title, private) in [("squat", false), ("squat-private", true)] {
        assert_eq!(
            exec_err_code(&schema, create(&ws.bob, &ws.server_id, title, private)).await,
            "FORBIDDEN"
        );
    }
    exec_ok(&schema, create(&ws.mona, &ws.server_id, "mods", true)).await;
    exec_ok(&schema, create(&ws.alice, &ws.server_id, "news", false)).await;

    ctx.join(
        &ws.other_server_id,
        &ws.bob,
        rim_core::server_member::model::ServerMemberRole::Member,
    )
    .await;
    exec_ok(
        &schema,
        create(&ws.bob, &ws.other_server_id, "elsewhere", false),
    )
    .await;
}

/// A member holds a fifth of the workspace's macro and pipeline caps, the Owner the full
/// per-user number; kicking or banning a member deletes their macros and pipelines there.
#[tokio::test]
async fn one_member_holds_a_share_of_the_automations_and_loses_them_when_removed() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("automations").await;
    let (schema, _) = ctx.build_with_limits(limits(|l| {
        l.macros_per_workspace = Some(5);
        l.pipelines_per_workspace = Some(5);
    }));
    let make_macro = |who: &Caller, name: &str| {
        who.request(
            CREATE_MACRO,
            json!({ "input": {
                "serverId": ws.server_id,
                "name": name,
                "steps": [{ "commandId": "chat.list_channels", "args": { "server_id": ws.server_id } }],
            } }),
        )
    };
    let make_pipeline = |who: &Caller, name: &str| {
        who.request(
            CREATE_PIPELINE,
            json!({ "input": {
                "serverId": ws.server_id,
                "name": name,
                "trigger": { "kind": "NEW_MESSAGE" },
                "actions": [{ "commandId": "chat.post_message" }],
            } }),
        )
    };

    for who in [&ws.bob, &ws.carol] {
        exec_ok(&schema, make_macro(who, "one")).await;
        exec_ok(&schema, make_pipeline(who, "one")).await;
        let (_, message) = exec_err(&schema, make_macro(who, "two")).await;
        assert_eq!(message, "user may have at most 1 macros per server");
        let (_, message) = exec_err(&schema, make_pipeline(who, "two")).await;
        assert_eq!(
            message,
            "quota exceeded: maximum 1 pipelines per user per server"
        );
    }
    exec_ok(&schema, make_macro(&ws.alice, "one")).await;
    exec_ok(&schema, make_macro(&ws.alice, "two")).await;

    let kick = json!({ "s": ws.server_id, "u": ws.bob.user_id.to_string() });
    exec_ok(&schema, ws.alice.request(KICK, kick)).await;
    let ban = json!({ "s": ws.server_id, "u": ws.carol.user_id.to_string() });
    exec_ok(&schema, ws.mona.request(BAN, ban)).await;
    for who in [&ws.bob, &ws.carol] {
        assert_eq!(
            count_rows(&ctx, "\"macro\"", "user_id", &ws.server_id, who).await,
            0
        );
        assert_eq!(
            count_rows(&ctx, "pipeline", "created_by", &ws.server_id, who).await,
            0
        );
    }
    assert_eq!(
        count_rows(&ctx, "\"macro\"", "user_id", &ws.server_id, &ws.alice).await,
        2
    );
}
