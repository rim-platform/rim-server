//! The health probes and `serverInfo` over HTTP on a real server: `/healthz` and `/readyz`
//! without a session or a matching `Origin`, `/readyz` turning 503 when a key file goes
//! missing, and `serverInfo` without a session, carrying the configured audit retention,
//! instance notice, terms URL and limits.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use reqwest::StatusCode;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use support::nodes::{S2S_TABLES, start};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_readiness_and_server_info_answer_without_a_session() {
    let ctx = support::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |config| {
        config.audit_retention_days = Some(90);
        config.instance_notice = Some("Run by the Example club.".to_string());
        config.terms_url = Some("https://rim.example.org/terms".to_string());
        config.limit_workspaces_per_user = Some(5);
        config.limit_channels_per_workspace = Some(30);
        config.limit_message_bytes = Some(8192);
    })
    .await;
    let http = reqwest::Client::new();
    let version = rim_server::config::VERSION;

    let healthz = http
        .get(format!("{}/healthz", node.origin))
        .header("origin", "https://elsewhere.example")
        .send()
        .await
        .expect("healthz");
    assert_eq!(healthz.status(), StatusCode::OK);
    let body: Value = healthz.json().await.expect("healthz json");
    assert_eq!(body, json!({ "status": "ok", "version": version }));

    let readyz = http
        .get(format!("{}/readyz", node.origin))
        .header("origin", "https://elsewhere.example")
        .send()
        .await
        .expect("readyz");
    assert_eq!(readyz.status(), StatusCode::OK);
    let body: Value = readyz.json().await.expect("readyz json");
    assert_eq!(
        body,
        json!({
            "status": "ok",
            "version": version,
            "checks": {
                "database": { "status": "ok" },
                "migrations": { "status": "ok" },
                "serverKey": { "status": "ok" },
                "frankingKey": { "status": "ok" },
            },
        })
    );

    let info: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({
            "query": "{ serverInfo { version apiVersion auditRetentionDays instanceNotice termsUrl \
                      limits { workspacesPerUser membersPerWorkspace channelsPerWorkspace \
                      messageBytes } } }"
        }))
        .send()
        .await
        .expect("serverInfo")
        .json()
        .await
        .expect("serverInfo json");
    assert_eq!(
        info,
        json!({ "data": { "serverInfo": {
            "version": version,
            "apiVersion": 1,
            "auditRetentionDays": 90,
            "instanceNotice": "Run by the Example club.",
            "termsUrl": "https://rim.example.org/terms",
            "limits": {
                "workspacesPerUser": 5,
                "membersPerWorkspace": null,
                "channelsPerWorkspace": 30,
                "messageBytes": 8192,
            },
        } } })
    );

    std::fs::remove_file(node.data.path().join("franking-key.json")).expect("remove key");
    // A readiness result is reused for one second, so the failure shows once it expires.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let readyz = loop {
        let readyz = http
            .get(format!("{}/readyz", node.origin))
            .send()
            .await
            .expect("readyz");
        if readyz.status() != StatusCode::OK || std::time::Instant::now() > deadline {
            break readyz;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    assert_eq!(readyz.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = readyz.json().await.expect("readyz json");
    assert_eq!(body["status"], "fail");
    assert_eq!(
        body["checks"]["frankingKey"],
        json!({ "status": "fail", "reason": "not readable" })
    );
    for check in ["database", "migrations", "serverKey"] {
        assert_eq!(body["checks"][check]["status"], "ok", "{check}");
    }

    let healthz = http
        .get(format!("{}/healthz", node.origin))
        .send()
        .await
        .expect("healthz");
    assert_eq!(healthz.status(), StatusCode::OK);

    node.stop().await;
}
