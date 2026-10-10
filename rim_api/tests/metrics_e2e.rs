//! A RIM server with `RIM_METRICS_ADDR` set: Prometheus text at `/metrics` on its own
//! listener, requests on the main port counted by route class, and nothing at `/metrics` on the
//! main port.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use support::nodes::{databases, start};

/// A loopback port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn scrape(client: &reqwest::Client, url: &str) -> String {
    let response = client.get(url).send().await.expect("scrape");
    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    response.text().await.expect("body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_are_served_on_their_own_listener() {
    let (first, _) = databases().await;
    let port = free_port();
    let node = start(&first, |config| {
        config.metrics_addr = Some(format!("127.0.0.1:{port}"));
    })
    .await;
    let client = reqwest::Client::new();
    let metrics_url = format!("http://127.0.0.1:{port}/metrics");

    let api = client
        .post(format!("{}/api", node.origin))
        .json(&serde_json::json!({ "query": "{ __typename }" }))
        .send()
        .await
        .expect("api");
    assert_eq!(api.status(), 200);
    let unsigned = client
        .post(format!("{}/s2s/v1/deliver", node.origin))
        .body("{}")
        .send()
        .await
        .expect("s2s");
    assert_eq!(unsigned.status(), 401);
    let on_main_port = client
        .get(format!("{}/metrics", node.origin))
        .send()
        .await
        .expect("main port");
    assert_eq!(on_main_port.status(), 404, "metrics stay off the API port");

    // The database gauges are sampled when the listener starts and every 30 seconds after.
    let mut body = String::new();
    for _ in 0..50 {
        body = scrape(&client, &metrics_url).await;
        if body.contains("rim_pending_migrations ") && body.contains("rim_relay_queue_items{") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for line in [
        format!(
            "rim_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        ),
        "rim_http_requests_total{route=\"api\",status=\"2xx\"}".to_string(),
        "rim_http_requests_total{route=\"s2s\",status=\"4xx\"}".to_string(),
        "rim_http_requests_total{route=\"other\",status=\"4xx\"}".to_string(),
        "rim_http_request_duration_seconds_bucket{route=\"api\",status=\"2xx\",le=\"".to_string(),
        "rim_s2s_requests_total{outcome=\"signature_invalid\"}".to_string(),
        "rim_pending_migrations 0".to_string(),
        "rim_relay_queue_items{state=\"queued\"}".to_string(),
        "rim_relay_queue_bytes{state=\"held\"}".to_string(),
        "rim_db_pool_connections ".to_string(),
        "rim_db_pool_connections_in_use ".to_string(),
        "rim_db_pool_max_connections ".to_string(),
    ] {
        assert!(body.contains(&line), "missing {line:?} in:\n{body}");
    }
    assert!(
        !body.contains("/s2s/v1/deliver"),
        "raw paths stay out of labels"
    );
    node.stop().await;
}
