//! The instance limits on a real server: one node capping workspaces per user, channels per
//! workspace, devices per user and message bytes refuses the write past each cap with
//! `VALIDATION` naming it, and allows the writes within it. With a one-hour probation, a new
//! account creates no workspace and sends no DM request to a stranger, a contact it added on
//! its own included, and `me` says when its probation ends; an instance operator is never on
//! probation, and the server's default workspace makes nobody known to a new account.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::ConnectionTrait;
use serde_json::json;
use uuid::Uuid;

use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::repository::UserRepository;
use rim_identity::testing::signing_key;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use support::graphql::{exec_err, exec_ok};
use support::identity::statement;
use support::nodes::{S2S_TABLES, start};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const CREATE_SERVER: &str =
    "mutation($name: String!) { createServer(input: { name: $name }) { id } }";
const CREATE_CHANNEL: &str = "mutation($s: ID!, $title: String!) { createConversation(input: \
     { serverId: $s, isChannel: true, title: $title }) { id } }";
const CHANNELS: &str =
    "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 100) { items { id title } } }";
const CREATE_DM: &str = "mutation($s: ID!, $u: ID!) { createConversation(input: \
     { serverId: $s, otherUserId: $u }) { id } }";
const PROBATION_ENDS: &str = "{ me { probationEndsAt } }";
const SEND: &str = "mutation($s: ID!, $c: ID!, $text: String!) { sendMessage(input: \
     { serverId: $s, conversationId: $c, text: $text }) { id } }";
const ADD_CONTACT: &str = "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, \
     relayUrls: $r) { principalId } }";
const ADD_DEVICE: &str =
    "mutation($s: SignedStatementInput!) { addDeviceKey(certificate: $s) { deviceId } }";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_cap_refuses_the_write_past_it_and_allows_those_within() {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |config| {
        config.key_login_audiences = Some(KEY_LOGIN_AUDIENCE.to_string());
        config.limit_workspaces_per_user = Some(1);
        config.limit_channels_per_workspace = Some(1);
        config.limit_devices_per_user = Some(1);
        config.limit_message_bytes = Some(16);
        config.probation_hours = Some(1);
    })
    .await;
    let schema = &node.schema;
    let refused = |(code, message): (String, String), expected: &str| {
        assert_eq!(code, "VALIDATION", "{message}");
        assert_eq!(message, expected);
    };

    // Ada's account is two hours old, past its probation.
    let ada = ctx.enrolled("ada", 1).await;
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE \"user\" SET created_at = now() - interval '2 hours' WHERE id = '{}'",
            ada.user.id
        ))
        .await
        .expect("age ada");

    // Workspaces per user: the first is allowed, the second refused.
    let server_id = exec_ok(
        schema,
        ada.caller
            .request(CREATE_SERVER, json!({ "name": "first" })),
    )
    .await["createServer"]["id"]
        .as_str()
        .expect("server id")
        .to_string();
    refused(
        exec_err(
            schema,
            ada.caller
                .request(CREATE_SERVER, json!({ "name": "second" })),
        )
        .await,
        "this instance allows 1 workspaces per user",
    );

    // Channels per workspace: the new workspace gets the one default channel its cap allows,
    // and another channel is refused.
    let channels = exec_ok(
        schema,
        ada.caller.request(CHANNELS, json!({ "s": server_id })),
    )
    .await;
    let items = channels["channels"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["title"], "general");
    let general = items[0]["id"].as_str().expect("general").to_string();
    refused(
        exec_err(
            schema,
            ada.caller
                .request(CREATE_CHANNEL, json!({ "s": server_id, "title": "news" })),
        )
        .await,
        "this instance allows 1 channels per workspace",
    );

    // Message bytes: 16 bytes are sent, 17 refused, counted in bytes rather than characters.
    let send = |text: String| {
        ada.caller
            .request(SEND, json!({ "s": server_id, "c": general, "text": text }))
    };
    exec_ok(schema, send("a".repeat(16))).await;
    for text in ["a".repeat(17), "é".repeat(9)] {
        refused(
            exec_err(schema, send(text)).await,
            "this instance allows 16 bytes of text per message",
        );
    }

    // Devices per user: enrolling the first device was allowed, a second is refused.
    let phone = ada
        .identity
        .device_cert(&ada.state, Uuid::now_v7(), &signing_key(200), "Phone");
    refused(
        exec_err(
            schema,
            ada.caller
                .request(ADD_DEVICE, json!({ "s": statement(&phone) })),
        )
        .await,
        "this instance allows 1 devices per user; revoke one first",
    );

    // Probation: a new account creates no workspace and says when its probation ends; Ada's
    // has ended.
    let newcomer = ctx.enrolled("newcomer", 3).await;
    newcomer.publish_card(schema).await;
    let fresh = &newcomer.caller;
    let in_probation = |(code, message): (String, String), action: &str| {
        assert_eq!(code, "VALIDATION", "{message}");
        let prefix = format!("a new account can {action} in ");
        let minutes = message
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(" minutes"))
            .unwrap_or_else(|| panic!("{message}"));
        assert!(
            (59..=60).contains(&minutes.parse::<u32>().expect("minutes")),
            "{message}"
        );
    };
    in_probation(
        exec_err(
            schema,
            fresh.request(CREATE_SERVER, json!({ "name": "mine" })),
        )
        .await,
        "create a workspace",
    );
    let ends = exec_ok(schema, fresh.request(PROBATION_ENDS, json!({}))).await;
    assert!(ends["me"]["probationEndsAt"].is_string(), "{ends}");
    let ended = exec_ok(schema, ada.caller.request(PROBATION_ENDS, json!({}))).await;
    assert!(ended["me"]["probationEndsAt"].is_null(), "{ended}");

    // A DM in the direct space to Ada, a stranger, is refused, and so it stays once the
    // newcomer adds her as a contact on their own; once both are in Ada's workspace she is no
    // stranger and the DM is allowed.
    let dm = || {
        fresh.request(
            CREATE_DM,
            json!({ "s": node.s2s.direct_space_id.to_string(), "u": ada.user.id.to_string() }),
        )
    };
    in_probation(
        exec_err(schema, dm()).await,
        "send a DM request to a stranger",
    );
    exec_ok(
        schema,
        fresh.request(
            ADD_CONTACT,
            json!({ "p": ada.identity.principal_id(), "r": ["https://relay.example"] }),
        ),
    )
    .await;
    in_probation(
        exec_err(schema, dm()).await,
        "send a DM request to a stranger",
    );
    SeaOrmServerMemberRepository::new(ctx.db.clone())
        .add_member(
            ServerId::from(Uuid::parse_str(&server_id).expect("uuid")),
            newcomer.user.id,
            ServerMemberRole::Member,
        )
        .await
        .expect("join");
    exec_ok(schema, dm()).await;

    // An instance operator is never on probation: an account as new as the newcomer's, granted
    // the role after its creation, has no probation end from then on, creates a workspace and
    // sends a DM request to Ada, a stranger.
    let operator = ctx.enrolled("operator", 4).await;
    operator.publish_card(schema).await;
    let op = &operator.caller;
    let before = exec_ok(schema, op.request(PROBATION_ENDS, json!({}))).await;
    assert!(before["me"]["probationEndsAt"].is_string(), "{before}");
    assert!(
        SeaOrmUserRepository::new(ctx.db.clone())
            .set_instance_operator(operator.user.id, true)
            .await
            .expect("grant")
    );
    let after = exec_ok(schema, op.request(PROBATION_ENDS, json!({}))).await;
    assert!(after["me"]["probationEndsAt"].is_null(), "{after}");
    exec_ok(schema, op.request(CREATE_SERVER, json!({ "name": "ops" }))).await;
    exec_ok(
        schema,
        op.request(
            CREATE_DM,
            json!({ "s": node.s2s.direct_space_id.to_string(), "u": ada.user.id.to_string() }),
        ),
    )
    .await;

    node.stop().await;
}

/// With a default workspace every new account joins, two new accounts that share only it are
/// strangers to each other during probation, for a DM in the direct space and for one in the
/// default workspace; once they share another workspace they are not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_workspace_makes_nobody_known_during_probation() {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let everyone = ctx.create_server("everyone").await;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |config| {
        config.key_login_audiences = Some(KEY_LOGIN_AUDIENCE.to_string());
        config.probation_hours = Some(1);
        config.default_workspace = Some(everyone.id.to_string());
    })
    .await;
    let schema = &node.schema;

    let amy = ctx.enrolled("amy", 1).await;
    amy.publish_card(schema).await;
    let fresh = &amy.caller;
    let ben = ctx.enrolled("ben", 2).await;
    let cal = ctx.create_user("cal").await;
    for user in [&amy.user, &ben.user, &cal] {
        ctx.add_member(&everyone, user, ServerMemberRole::Member)
            .await;
    }
    let direct = node.s2s.direct_space_id.to_string();
    let in_everyone = everyone.id.to_string();
    let dm = |space: &str, other: &rim_core::user::model::User| {
        fresh.request(CREATE_DM, json!({ "s": space, "u": other.id.to_string() }))
    };
    for (space, other) in [
        (&direct, &ben.user),
        (&in_everyone, &ben.user),
        (&in_everyone, &cal),
    ] {
        let (code, message) = exec_err(schema, dm(space, other)).await;
        assert_eq!(code, "VALIDATION", "{message}");
        assert!(
            message.starts_with("a new account can send a DM request to a stranger in "),
            "{space} {message}"
        );
    }

    let team = ctx.create_server("team").await;
    for user in [&amy.user, &ben.user, &cal] {
        ctx.add_member(&team, user, ServerMemberRole::Member).await;
    }
    exec_ok(schema, dm(&direct, &ben.user)).await;
    exec_ok(schema, dm(&in_everyone, &cal)).await;

    node.stop().await;
}
