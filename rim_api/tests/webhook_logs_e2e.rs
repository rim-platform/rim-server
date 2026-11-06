//! A webhook's token in its URL path reaches no log line, span, audit row or metric: every outcome
//! of the route is driven against a real server with everything the process logs captured at the
//! `trace` level, `log` lines from Actix included. Its own binary, since a logger is global.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test webhook_logs_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::io::Write;
use std::sync::{Arc, Mutex};

use reqwest::StatusCode;
use rim_auth::session_token::hash_token;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use support::TestCtx;
use support::graphql::{Caller, exec_ok};
use support::nodes::{S2S_TABLES, start};
use support::roles::create_channel;

const CREATE_BOT: &str = r#"mutation($input: CreateBotInput!) {
  createBot(input: $input) { bot { userId } }
}"#;
const CREATE: &str = r#"mutation($input: CreateIncomingWebhookInput!) {
  createIncomingWebhook(input: $input) { token webhook { id } }
}"#;
const REVOKE: &str = "mutation($id: ID!) { revokeIncomingWebhook(id: $id) { id } }";
const TIMEOUT: &str = r#"mutation($s: ID!, $u: ID!, $until: DateTime!) {
  timeoutMember(serverId: $s, userId: $u, until: $until) { userId }
}"#;

/// Everything written to the log.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("lock")).into_owned()
    }

    /// Install it as the process's logger: every level, with spans as they open and close, and
    /// the `log` crate bridged. The test's own HTTP client is left out, since it knows the URL.
    fn install(&self) {
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_writer(self.clone())
            .with_filter(filter_fn(|metadata| {
                let target = metadata.target();
                !["reqwest", "hyper_util", "hyper", "h2", "rustls"]
                    .iter()
                    .any(|client| target.starts_with(client))
            }));
        tracing_subscriber::registry()
            .with(layer)
            .try_init()
            .expect("the only logger of this binary");
    }
}

async fn gql(node: &support::nodes::Node, who: &Caller, query: &str, variables: Value) -> Value {
    exec_ok(&node.schema, who.request(query, variables)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_outcome_of_the_route_lets_the_token_out() {
    let captured = Captured::default();
    captured.install();

    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |_| {}).await;
    let r = ctx.roles("logs").await;
    let http = reqwest::Client::new();

    let bot = gql(
        &node,
        &r.alice,
        CREATE_BOT,
        json!({ "input": {
            "serverId": r.server_id, "name": "Deploys", "runBy": "Ada", "readsGoTo": "Nowhere",
        }}),
    )
    .await["createBot"]["bot"]["userId"]
        .as_str()
        .expect("bot")
        .to_string();
    let ops = create_channel(&node.schema, &r.alice, &r.server_id, "ops", false).await;
    let created = gql(
        &node,
        &r.alice,
        CREATE,
        json!({ "input": { "conversationId": ops, "botUserId": bot, "preset": "PLAIN" } }),
    )
    .await;
    let token = created["createIncomingWebhook"]["token"]
        .as_str()
        .expect("token")
        .to_string();
    let webhook_id = created["createIncomingWebhook"]["webhook"]["id"].clone();
    let hook_url = format!("{}/api/hooks/{token}", node.origin);

    let post = async |body: Vec<u8>| {
        http.post(&hook_url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("a post")
            .status()
    };
    let text = |text: &str| json!({ "text": text }).to_string().into_bytes();

    // Posted, ignored, unreadable and too large.
    assert_eq!(post(text("hello")).await, StatusCode::OK);
    assert_eq!(post(text("  ")).await, StatusCode::ACCEPTED);
    assert_eq!(post(b"not json".to_vec()).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        post(vec![b' '; 1024 * 1024 + 1]).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );

    // Paths the route does not answer: another method, a longer path, a query.
    let got = http.get(&hook_url).send().await.expect("a get").status();
    assert_eq!(got, StatusCode::METHOD_NOT_ALLOWED);
    let longer = http
        .post(format!("{hook_url}/more"))
        .send()
        .await
        .expect("a longer path")
        .status();
    assert_eq!(longer, StatusCode::NOT_FOUND);

    // Refused by the command.
    let until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    gql(
        &node,
        &r.alice,
        TIMEOUT,
        json!({ "s": r.server_id, "u": bot, "until": until }),
    )
    .await;
    assert_eq!(post(text("timed out")).await, StatusCode::FORBIDDEN);

    // Over the limit: the posts above were charged too.
    let mut last = StatusCode::OK;
    for _ in 0..30 {
        last = post(text("again")).await;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);

    // Revoked, then a token of another shape in the same place.
    gql(&node, &r.alice, REVOKE, json!({ "id": webhook_id })).await;
    assert_eq!(post(text("late")).await, StatusCode::UNAUTHORIZED);
    let unknown = format!("rimhook_{}", "z".repeat(43));
    let refused = http
        .post(format!("{}/api/hooks/{unknown}", node.origin))
        .body("{}")
        .send()
        .await
        .expect("an unknown token")
        .status();
    assert_eq!(refused, StatusCode::UNAUTHORIZED);

    let log = captured.text();
    // The capture sees what matters: the server's `log` lines, the audit line of the post and the
    // webhook's id, so its silence about the token means something.
    assert!(log.contains("actix_server"), "the log bridge is on");
    assert!(log.contains("chat.post_message"), "the audit line is seen");
    assert!(
        log.contains(webhook_id.as_str().expect("id")),
        "the management spans are seen"
    );
    for secret in [
        token.as_str(),
        hash_token(&token).as_str(),
        unknown.as_str(),
        "rimhook_",
        "/api/hooks/",
    ] {
        assert!(!log.contains(secret), "the log holds {secret}");
    }

    // Nor does an audit row hold it.
    let rows = ctx
        .conn
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM command_audit_log \
             WHERE args_redacted::text LIKE '%rimhook_%' OR error_kind LIKE '%rimhook_%'",
        ))
        .await
        .expect("audit")
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(rows, 0);

    node.stop().await;
}
