//! Through-the-stack e2e tests for the server's default workspace: with
//! `RIM_DEFAULT_WORKSPACE` set, every signup makes the new account a Member of that workspace
//! with its public channels, and `serverInfo.defaultWorkspace` names it; unset, a signup joins
//! nothing.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use chrono::Utc;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::user::repository::UserRepository;
use rim_identity::StatementVersion;
use rim_identity::testing::TestIdentity;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, signed_out, signup_input};
use support::registration::seed_operator_invite;

const SIGNUP_WITH_INVITE: &str = r#"
mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId serverId } }"#;

const CREATE_CHANNEL: &str = r#"
mutation Create($input: CreateConversationInput!) { createConversation(input: $input) { id } }"#;

const MY_SERVERS: &str = "query { myServers { id } }";

const JOINED_CHANNELS: &str = r#"
query Channels($serverId: ID!) {
  channels(serverId: $serverId, dir: AFTER, limit: 50, filter: { joinedOnly: true }) {
    items { id title isMember }
  }
}"#;

const SERVER_INFO: &str = "{ serverInfo { defaultWorkspace } }";

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }"#;

const SET_LEVEL: &str = r#"
mutation SetLevel($serverId: ID!, $conversationId: ID!, $level: NotificationLevel) {
  setConversationNotificationLevel(serverId: $serverId, conversationId: $conversationId,
                                   level: $level) { effectiveLevel }
}"#;

const SETTINGS: &str = r#"
query Settings($serverId: ID!) { myNotificationSettings(serverId: $serverId) { defaultLevel } }"#;

/// A workspace with an owner, a public channel `general` and a private channel `staff`.
struct Workspace {
    id: String,
    general: String,
    staff: String,
    owner: Caller,
}

async fn workspace(ctx: &TestCtx, schema: &AppSchema) -> Workspace {
    let owner = ctx.create_user("dw-owner").await;
    let server = ctx.create_server("Lobby").await;
    ctx.add_owner(&server, &owner).await;
    let owner = ctx.caller(&owner).await;
    let id = server.id.to_string();
    let channel = |title: &'static str, is_private: bool| {
        owner.request(
            CREATE_CHANNEL,
            json!({ "input": {
                "serverId": id, "isChannel": true, "title": title, "isPrivate": is_private,
            } }),
        )
    };
    let general = exec_ok(schema, channel("general", false)).await["createConversation"]["id"]
        .as_str()
        .expect("general id")
        .to_string();
    let staff = exec_ok(schema, channel("staff", true)).await["createConversation"]["id"]
        .as_str()
        .expect("staff id")
        .to_string();
    Workspace {
        id,
        general,
        staff,
        owner,
    }
}

/// A caller acting as the account `user_id` a signup returned.
async fn caller_for(ctx: &TestCtx, user_id: &Value) -> Caller {
    let id: uuid::Uuid = user_id
        .as_str()
        .expect("userId")
        .parse()
        .expect("uuid userId");
    let user = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(id.into())
        .await
        .expect("user lookup")
        .expect("the new account");
    ctx.caller(&user).await
}

/// The role `user_id` holds in workspace `server_id`, `None` when not a member.
async fn member_role(ctx: &TestCtx, server_id: &str, user_id: &Value) -> Option<String> {
    let row = ctx
        .conn
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT member_role::text AS role FROM server_member \
             WHERE server_id = $1::uuid AND user_id = $2::uuid",
            [server_id.into(), user_id.as_str().expect("userId").into()],
        ))
        .await
        .expect("query membership");
    row.map(|r| r.try_get::<String>("", "role").expect("role"))
}

/// The ids of the workspaces `who` belongs to.
async fn my_servers(schema: &AppSchema, who: &Caller) -> Vec<String> {
    exec_ok(schema, who.request(MY_SERVERS, json!({}))).await["myServers"]
        .as_array()
        .expect("myServers")
        .iter()
        .map(|s| s["id"].as_str().expect("id").to_string())
        .collect()
}

/// The new account `user_id` is a Member of `ws`, lists it among its workspaces, and has
/// joined `general` but not `staff`.
async fn assert_landed(ctx: &TestCtx, schema: &AppSchema, ws: &Workspace, user_id: &Value) {
    assert_eq!(
        member_role(ctx, &ws.id, user_id).await.as_deref(),
        Some("member")
    );
    let who = caller_for(ctx, user_id).await;
    assert_eq!(my_servers(schema, &who).await, vec![ws.id.clone()]);
    let joined = exec_ok(
        schema,
        who.request(JOINED_CHANNELS, json!({ "serverId": ws.id })),
    )
    .await["channels"]["items"]
        .as_array()
        .expect("channel items")
        .clone();
    let general = joined
        .iter()
        .find(|c| c["id"] == ws.general.as_str())
        .expect("the public channel is joined");
    assert_eq!(general["isMember"], true);
    assert!(
        joined.iter().all(|c| c["id"] != ws.staff.as_str()),
        "the private channel is not joined"
    );
}

fn password_signup(username: &str, extra: Value) -> async_graphql::Request {
    let mut input = json!({
        "username": username,
        "email": format!("{username}@test.example.com"),
        "password": "ValidPass123!",
    });
    if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        input.extend(extra.clone());
    }
    anonymous_request(SIGNUP_WITH_INVITE, json!({ "input": input }))
}

/// With a default workspace, a password signup in open mode, a key-only signup in open mode
/// and a password signup with an instance invite each land the new account in the workspace
/// as a Member with its public channels; the signup payload's `serverId` stays null, and
/// `serverInfo.defaultWorkspace` names the workspace.
#[tokio::test]
async fn every_signup_joins_the_default_workspace_and_its_public_channels() {
    let ctx = TestCtx::new().await;
    let probe = ctx.build_schema(RegistrationMode::Open).await;
    let ws = workspace(&ctx, &probe).await;
    let server_id: uuid::Uuid = ws.id.parse().expect("workspace uuid");
    let open = ctx.build_schema_with_default_workspace(server_id.into(), RegistrationMode::Open);
    let invite_only =
        ctx.build_schema_with_default_workspace(server_id.into(), RegistrationMode::InviteOnly);

    let info = exec_ok(&open, anonymous_request(SERVER_INFO, json!({}))).await;
    assert_eq!(info["serverInfo"]["defaultWorkspace"], json!(ws.id));

    let data = exec_ok(&open, password_signup("dw-open", json!({}))).await;
    assert!(data["signupWithInvite"]["serverId"].is_null());
    assert_landed(&ctx, &open, &ws, &data["signupWithInvite"]["userId"]).await;

    let id = TestIdentity::with_version(61, StatementVersion::V2);
    let input = signup_input(&open, "dw-key", &id, 71, json!({})).await;
    let data = exec_ok(
        &open,
        signed_out(SIGNUP_WITH_KEY, json!({ "input": input })),
    )
    .await;
    assert!(data["signupWithKey"]["serverId"].is_null());
    assert_landed(&ctx, &open, &ws, &data["signupWithKey"]["userId"]).await;

    seed_operator_invite(
        &ctx.db,
        "rim_iinv_default_workspace",
        Utc::now() + chrono::Duration::minutes(15),
    )
    .await;
    let data = exec_ok(
        &invite_only,
        password_signup(
            "dw-invited",
            json!({ "instanceInviteToken": "rim_iinv_default_workspace" }),
        ),
    )
    .await;
    assert!(data["signupWithInvite"]["serverId"].is_null());
    assert_landed(&ctx, &invite_only, &ws, &data["signupWithInvite"]["userId"]).await;
}

/// Without a default workspace a signup joins no workspace, even with a workspace holding a
/// public channel on the server, and `serverInfo.defaultWorkspace` is null.
#[tokio::test]
async fn without_a_default_workspace_a_signup_joins_nothing() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ws = workspace(&ctx, &schema).await;

    let info = exec_ok(&schema, anonymous_request(SERVER_INFO, json!({}))).await;
    assert!(info["serverInfo"]["defaultWorkspace"].is_null());

    let data = exec_ok(&schema, password_signup("dw-alone", json!({}))).await;
    let user_id = &data["signupWithInvite"]["userId"];
    assert_eq!(member_role(&ctx, &ws.id, user_id).await, None);
    let who = caller_for(&ctx, user_id).await;
    assert!(my_servers(&schema, &who).await.is_empty());
}

/// The users holding a notification for `message_id`, by id.
async fn notified(ctx: &TestCtx, message_id: &str) -> Vec<String> {
    ctx.conn
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT user_id::text AS user_id FROM user_notification \
             WHERE message_id = $1::uuid ORDER BY user_id",
            [message_id.into()],
        ))
        .await
        .expect("query notifications")
        .iter()
        .map(|r| r.try_get::<String>("", "user_id").expect("user_id"))
        .collect()
}

/// In the default workspace a member who never saved notification settings is at `MENTIONS`:
/// a plain message in a public channel stores no notification for them, a mention does, and a
/// member who sets the channel to `ALL` is told about every message.
#[tokio::test]
async fn the_default_workspace_notifies_plain_messages_only_to_members_who_chose_all() {
    let ctx = TestCtx::new().await;
    let probe = ctx.build_schema(RegistrationMode::Open).await;
    let ws = workspace(&ctx, &probe).await;
    let server_id: uuid::Uuid = ws.id.parse().expect("workspace uuid");
    let schema = ctx.build_schema_with_default_workspace(server_id.into(), RegistrationMode::Open);

    let mut members = Vec::new();
    for username in ["dw-quiet", "dw-loud"] {
        let data = exec_ok(&schema, password_signup(username, json!({}))).await;
        let user_id = data["signupWithInvite"]["userId"].clone();
        members.push((
            user_id.as_str().expect("userId").to_string(),
            caller_for(&ctx, &user_id).await,
        ));
    }
    let (quiet_id, quiet) = &members[0];
    let (loud_id, loud) = &members[1];

    let settings = exec_ok(
        &schema,
        quiet.request(SETTINGS, json!({ "serverId": ws.id })),
    )
    .await;
    assert_eq!(
        settings["myNotificationSettings"]["defaultLevel"],
        "MENTIONS"
    );
    let level = exec_ok(
        &schema,
        loud.request(
            SET_LEVEL,
            json!({ "serverId": ws.id, "conversationId": ws.general, "level": "ALL" }),
        ),
    )
    .await;
    assert_eq!(
        level["setConversationNotificationLevel"]["effectiveLevel"],
        "ALL"
    );

    let send = |text: String| {
        ws.owner.request(
            SEND_MESSAGE,
            json!({ "input": {
                "serverId": ws.id, "conversationId": ws.general, "text": text,
            } }),
        )
    };
    let plain = exec_ok(&schema, send("hello everyone".to_string())).await;
    let plain = plain["msg"]["id"].as_str().expect("id");
    assert_eq!(notified(&ctx, plain).await, vec![loud_id.clone()]);

    let mention = exec_ok(&schema, send(format!("ping <@{quiet_id}>"))).await;
    let mention = mention["msg"]["id"].as_str().expect("id");
    let mut expected = vec![quiet_id.clone(), loud_id.clone()];
    expected.sort();
    assert_eq!(notified(&ctx, mention).await, expected);
}

#[tokio::test]
async fn older_accounts_self_join_and_an_unban_restores_only_public_channels() {
    let ctx = TestCtx::new().await;
    let initial = ctx.build_schema(RegistrationMode::Open).await;
    let ws = workspace(&ctx, &initial).await;
    let schema = ctx.build_schema_with_default_workspace(
        ws.id.parse::<uuid::Uuid>().unwrap().into(),
        RegistrationMode::Open,
    );
    let user = ctx.create_user("dw-older").await;
    let caller = ctx.caller(&user).await;
    let join = "mutation { joinDefaultWorkspace }";
    let joined = exec_ok(&schema, caller.request(join, json!({}))).await;
    assert_eq!(joined["joinDefaultWorkspace"], ws.id);
    let ban = "mutation($server: ID!, $user: ID!) { banMember(serverId: $server, userId: $user) { userId } }";
    let vars = json!({"server": ws.id, "user": user.id.to_string()});
    exec_ok(&schema, ws.owner.request(ban, vars.clone())).await;
    let refused = schema.execute(caller.request(join, json!({}))).await;
    assert!(!refused.errors.is_empty());
    let unban =
        "mutation($server: ID!, $user: ID!) { unbanMember(serverId: $server, userId: $user) }";
    exec_ok(&schema, ws.owner.request(unban, vars)).await;
    let joined = exec_ok(&schema, caller.request(join, json!({}))).await;
    assert_eq!(joined["joinDefaultWorkspace"], ws.id);
    let channels = exec_ok(
        &schema,
        caller.request(JOINED_CHANNELS, json!({"serverId":ws.id})),
    )
    .await;
    let ids: Vec<_> = channels["channels"]["items"]
        .as_array()
        .expect("channels")
        .iter()
        .map(|channel| channel["id"].as_str().expect("id"))
        .collect();
    assert!(ids.contains(&ws.general.as_str()));
    assert!(!ids.contains(&ws.staff.as_str()));
}
