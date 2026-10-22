//! The edge limits on a real server: the per-address request limit on `/api` routes with
//! `/healthz` outside it, the GraphQL batch cap and body caps, the per-caller mutation budget,
//! and the IP deny list loaded at boot.
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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use std::sync::Arc;

use rim_infra_db::sea_orm_impl::ip_block::SeaOrmIpBlockRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::ip_block::DefaultIpBlockService;
use rim_services::ip_block::dto::BlockIpCmd;
use rim_services::ip_block::service::IpBlockService;
use rim_services::operator::OperatorCaller;
use support::nodes::{Node, S2S_TABLES, start};

/// A node on `DATABASE_URL` with `tweak` applied, its S2S tables cleared first.
async fn node(tweak: impl FnOnce(&mut rim_server::config::RIMConfig)) -> Node {
    let ctx = support::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    start(&url, tweak).await
}

const SERVER_INFO: &str = "{ serverInfo { version } }";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_address_over_its_limit_gets_429_and_a_large_batch_gets_400() {
    let node = node(|config| config.rate_requests_per_min_per_ip = Some(3)).await;
    let http = reqwest::Client::new();
    let api = format!("{}/api", node.origin);

    let batch: Vec<Value> = (0..21).map(|_| json!({ "query": SERVER_INFO })).collect();
    let res = http.post(&api).json(&batch).send().await.expect("batch");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.expect("batch json");
    assert_eq!(body, json!({ "error": "batch_too_large" }));

    let res = http
        .post(&api)
        .json(&json!({ "query": SERVER_INFO }))
        .send()
        .await
        .expect("second");
    assert_eq!(res.status(), StatusCode::OK);
    let res = http
        .get(format!("{}/api/sdl", node.origin))
        .send()
        .await
        .expect("third");
    assert_eq!(res.status(), StatusCode::OK);

    let res = http
        .post(&api)
        .json(&json!({ "query": SERVER_INFO }))
        .send()
        .await
        .expect("fourth");
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        res.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("20")
    );
    let body: Value = res.json().await.expect("429 json");
    assert_eq!(
        body,
        json!({ "error": "rate_limited", "retryAfterSeconds": 20 })
    );

    let healthz = http
        .get(format!("{}/healthz", node.origin))
        .send()
        .await
        .expect("healthz");
    assert_eq!(healthz.status(), StatusCode::OK);

    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graphql_body_past_its_caps_gets_413_and_runs_nothing() {
    let node = node(|_| {}).await;
    let http = reqwest::Client::new();
    let api = format!("{}/api", node.origin);
    let post = |body: Vec<u8>| {
        http.post(&api)
            .header("content-type", "application/json")
            .body(body)
            .send()
    };

    // Over 12 MiB, from the declared length alone, before any of the body is read.
    let authority = node.origin.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("connect");
    let request = format!(
        "POST /api HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\n\r\n",
        12 * 1024 * 1024 + 1
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut raw),
    )
    .await
    .expect("the server answers and closes")
    .expect("read");
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.starts_with("HTTP/1.1 413"), "{raw}");
    assert!(raw.contains(r#""error":"too_large""#), "{raw}");

    let many = format!(
        r#"{{"query":"{SERVER_INFO}","variables":{{"x":[{}0]}}}}"#,
        "0,".repeat(300_000)
    );
    let long = serde_json::to_vec(&json!({
        "query": format!("{{ {} }}", "serverInfo { version } ".repeat(10_000)),
    }))
    .expect("json");
    for (what, body) in [
        ("too many values", many.into_bytes()),
        ("documents too long", long),
    ] {
        let res = post(body).await.expect("request");
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE, "{what}");
        let body: Value = res.json().await.expect("413 json");
        assert_eq!(body["error"], "too_large", "{what}");
    }

    let res = post(serde_json::to_vec(&json!({ "query": SERVER_INFO })).expect("json"))
        .await
        .expect("small");
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("json");
    assert!(body["data"]["serverInfo"]["version"].is_string(), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_fields_past_the_budget_answer_rate_limited() {
    let node = node(|config| config.rate_mutations_per_min_per_session = Some(2)).await;
    let http = reqwest::Client::new();
    let api = format!("{}/api", node.origin);

    let res: Value = http
        .post(&api)
        .json(&json!({ "query": "mutation { logout }" }))
        .send()
        .await
        .expect("first")
        .json()
        .await
        .expect("first json");
    // Signed out, logout runs and refuses: the field was counted, not rate-limited.
    assert_eq!(
        res["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{res}"
    );

    // Two fields with one token left: refused whole, before anything runs.
    let res: Value = http
        .post(&api)
        .json(&json!({ "query": "mutation { a: logout b: logout }" }))
        .send()
        .await
        .expect("second")
        .json()
        .await
        .expect("second json");
    assert_eq!(
        res["errors"][0]["extensions"]["code"], "RATE_LIMITED",
        "{res}"
    );
    assert_eq!(
        res["errors"][0]["extensions"]["retryAfterSeconds"], 30,
        "{res}"
    );

    let res: Value = http
        .post(&api)
        .json(&json!({ "query": SERVER_INFO }))
        .send()
        .await
        .expect("query")
        .json()
        .await
        .expect("query json");
    assert!(
        res.get("errors").is_none(),
        "queries are not counted: {res}"
    );

    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_address_gets_403_on_every_route_but_healthz() {
    let ctx = support::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let blocks = DefaultIpBlockService::new(
        Arc::new(SeaOrmIpBlockRepository::new(ctx.db.clone())),
        Arc::new(SeaOrmUserRepository::new(ctx.db.clone())),
    );
    blocks
        .block(
            OperatorCaller::CommandLine,
            BlockIpCmd {
                cidr: "127.0.0.1/32".to_string(),
                reason: "e2e".to_string(),
                expires_in_days: Some(1),
            },
        )
        .await
        .expect("block");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |_| {}).await;
    let http = reqwest::Client::new();

    let healthz = http
        .get(format!("{}/healthz", node.origin))
        .send()
        .await
        .expect("healthz");
    assert_eq!(healthz.status(), StatusCode::OK);

    for (method, path) in [
        (reqwest::Method::POST, "/api"),
        (reqwest::Method::GET, "/api/sdl"),
        (reqwest::Method::GET, "/readyz"),
        (reqwest::Method::GET, "/.well-known/rim-server"),
        (reqwest::Method::POST, "/s2s/v1/deliver"),
    ] {
        let res = http
            .request(method.clone(), format!("{}{path}", node.origin))
            .json(&json!({ "query": SERVER_INFO }))
            .send()
            .await
            .expect("request");
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{method} {path}");
        let body: Value = res.json().await.expect("403 json");
        assert_eq!(body, json!({ "error": "blocked" }), "{method} {path}");
    }

    assert!(
        blocks
            .unblock(OperatorCaller::CommandLine, "127.0.0.1".to_string())
            .await
            .expect("unblock")
    );
    node.stop().await;
}
