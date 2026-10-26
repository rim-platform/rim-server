//! Instance feature flags on a real server started with
//! `RIM_FEATURES_DISABLED=attachments,custom_emoji,avatars`: `serverInfo.features` and the
//! discovery document report them, the attachment and avatar routes, a custom emoji mutation and
//! setting an avatar answer `FEATURE_DISABLED`, a stored avatar URL reads as null, and the
//! descriptor still verifies.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use rim_identity::SignedStatement;
use rim_s2s::{ServerKeyState, verify_descriptor};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use support::nodes::start;
use support::relay::{RESET, open_registration, sign_up, signed, user_id};

const SERVER_INFO: &str = "{ serverInfo { features { key state reason } } }";

const CREATE_EMOJI: &str = r#"
mutation Emoji($input: CreateCustomEmojiInput!) { createCustomEmoji(input: $input) { id } }"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_features_are_reported_and_refused() {
    let ctx = support::TestCtx::new().await;
    ctx.conn.execute_unprepared(RESET).await.expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |config| {
        open_registration(config);
        config.features_disabled = Some("attachments, custom_emoji, avatars".to_string());
    })
    .await;
    let http = reqwest::Client::new();

    // serverInfo, without a session.
    let info: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": SERVER_INFO }))
        .send()
        .await
        .expect("serverInfo")
        .json()
        .await
        .expect("json");
    let features = info["data"]["serverInfo"]["features"]
        .as_array()
        .expect("features")
        .clone();
    let state = |key: &str| {
        features
            .iter()
            .find(|f| f["key"] == key)
            .unwrap_or_else(|| panic!("{key} in {info}"))
            .clone()
    };
    assert_eq!(
        state("attachments"),
        json!({ "key": "attachments", "state": "DISABLED", "reason": null })
    );
    assert_eq!(state("custom_emoji")["state"], "DISABLED");
    assert_eq!(state("avatars")["state"], "DISABLED");
    assert_eq!(state("email")["state"], "UNAVAILABLE");
    assert_eq!(state("email")["reason"], "no mail server is set up");
    for key in ["link_previews", "workspace_plugins", "ai_commands"] {
        assert_eq!(state(key)["state"], "ENABLED", "{key}");
    }

    // The discovery document carries the same states beside a descriptor that still verifies.
    let doc: Value = http
        .get(format!("{}/.well-known/rim-server", node.origin))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json");
    assert_eq!(
        doc["features"]["attachments"],
        json!({ "state": "disabled" })
    );
    assert_eq!(
        doc["features"]["custom_emoji"],
        json!({ "state": "disabled" })
    );
    assert_eq!(doc["features"]["email"]["state"], "unavailable");
    assert_eq!(
        doc["features"]["ai_commands"],
        json!({ "state": "enabled" })
    );
    let log: Vec<SignedStatement> = doc["keyLog"]
        .as_array()
        .expect("log")
        .iter()
        .map(signed)
        .collect();
    let key_state = ServerKeyState::replay(&log).expect("log replays");
    let descriptor = verify_descriptor(&key_state, &signed(&doc["descriptor"])).expect("verifies");
    assert_eq!(descriptor.server_id, node.id());
    // The handle domain defaults to the host of the public origin.
    assert_eq!(descriptor.handle_domain.as_deref(), Some("127.0.0.1"));
    assert!(
        !doc["descriptor"]["payload"]
            .as_str()
            .expect("payload")
            .contains("features")
    );

    // A signed-in member is refused the upload and the emoji mutation.
    let person = sign_up(&node, "frida", 71).await;
    let upload = person
        .http
        .post(format!("{}/api/files/upload", node.origin))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", person.session, person.csrf),
        )
        .header("x-csrf-token", &person.csrf)
        .header("content-type", "multipart/form-data; boundary=rimboundary")
        .body(
            "--rimboundary\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--rimboundary--\r\n",
        )
        .send()
        .await
        .expect("upload");
    assert_eq!(upload.status().as_u16(), 403);
    let body: Value = upload.json().await.expect("json");
    assert_eq!(body["code"], "FEATURE_DISABLED");
    assert_eq!(body["feature"], "attachments");

    let direct = person.direct_space().await;
    let code = person
        .err(
            CREATE_EMOJI,
            json!({ "input": { "serverId": direct, "name": "party", "imageBase64": "AA==" } }),
        )
        .await;
    assert_eq!(code, "FEATURE_DISABLED");

    // Avatar upload and download answer 403 with the feature, and setting one is refused.
    let cookie = format!("rim_session={}; rim_csrf={}", person.session, person.csrf);
    let avatar = person
        .http
        .post(format!("{}/api/files/avatar", node.origin))
        .header("cookie", &cookie)
        .header("x-csrf-token", &person.csrf)
        .header("content-type", "multipart/form-data; boundary=rimboundary")
        .body(
            "--rimboundary\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\npng\r\n--rimboundary--\r\n",
        )
        .send()
        .await
        .expect("avatar upload");
    let download = person
        .http
        .get(format!("{}/api/files/avatars/u/avatar.png", node.origin))
        .header("cookie", &cookie)
        .send()
        .await
        .expect("avatar download");
    for resp in [avatar, download] {
        assert_eq!(resp.status().as_u16(), 403);
        let body: Value = resp.json().await.expect("json");
        assert_eq!(body["code"], "FEATURE_DISABLED");
        assert_eq!(body["feature"], "avatars");
    }
    let code = person
        .err(
            "mutation { updateMyProfile(input: { avatarUrl: \"/api/files/avatars/u/avatar.png\" }) { id } }",
            json!({}),
        )
        .await;
    assert_eq!(code, "FEATURE_DISABLED");

    // An avatar stored before the switch reads as null.
    let id = user_id(&node, &person.principal()).await;
    node.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE "user" SET avatar_url = '/api/files/avatars/u/avatar.png' WHERE id = $1"#,
            [uuid::Uuid::from(id).into()],
        ))
        .await
        .expect("set avatar");
    let me = person.ok("{ me { avatarUrl } }", json!({})).await;
    assert_eq!(me["me"]["avatarUrl"], Value::Null);

    node.stop().await;
    ctx.conn.close().await.expect("close");
}
