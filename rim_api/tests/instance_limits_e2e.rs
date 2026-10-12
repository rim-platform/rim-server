//! The instance limits on a real server: one node capping workspaces per user, channels per
//! workspace, devices per user and message bytes refuses the write past each cap with
//! `VALIDATION` naming it, and allows the writes within it.
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

use rim_identity::testing::signing_key;
use support::graphql::{exec_err, exec_ok};
use support::identity::statement;
use support::nodes::{S2S_TABLES, start};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const CREATE_SERVER: &str =
    "mutation($name: String!) { createServer(input: { name: $name }) { id } }";
const CREATE_CHANNEL: &str = "mutation($s: ID!, $title: String!) { createConversation(input: \
     { serverId: $s, isChannel: true, title: $title }) { id } }";
const CHANNELS: &str =
    "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 100) { items { title } } }";
const SEND: &str = "mutation($s: ID!, $c: ID!, $text: String!) { sendMessage(input: \
     { serverId: $s, conversationId: $c, text: $text }) { id } }";
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
    })
    .await;
    let schema = &node.schema;
    let refused = |(code, message): (String, String), expected: &str| {
        assert_eq!(code, "VALIDATION", "{message}");
        assert_eq!(message, expected);
    };

    // Workspaces per user: the first is allowed, the second refused.
    let ada = ctx.enrolled(schema, "ada", 1).await;
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
    assert_eq!(
        channels["channels"]["items"],
        json!([{ "title": "general" }])
    );
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
    let general = exec_ok(
        schema,
        ada.caller.request(
            "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 100) { items { id } } }",
            json!({ "s": server_id }),
        ),
    )
    .await["channels"]["items"][0]["id"]
        .as_str()
        .expect("general")
        .to_string();
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

    node.stop().await;
}
