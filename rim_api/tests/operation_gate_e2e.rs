//! The checks that classify a GraphQL operation, on a real server: a protected mutation hidden
//! in nested fragments still needs the CSRF header.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use support::nodes::{Node, S2S_TABLES, start};
use support::sessions::{LOGIN, PASSWORD};

/// A mutation exempt from CSRF in sight, `updateMyProfile` two named fragments down.
const NESTED_PROTECTED: &str = r#"mutation {
  logout @skip(if: true)
  ...Outer
}
fragment Outer on Mutation { ...Inner }
fragment Inner on Mutation {
  updateMyProfile(input: { statusText: "changed without CSRF" }) { id }
}"#;

const STATUS_TEXT: &str = "{ me { statusText } }";

/// A server, and a password user signed in to it over HTTP.
struct SignedIn {
    node: Node,
    http: reqwest::Client,
    session: String,
    csrf: String,
}

impl SignedIn {
    async fn new() -> Self {
        let ctx = support::TestCtx::new().await;
        ctx.conn
            .execute_unprepared(S2S_TABLES)
            .await
            .expect("truncate");
        ctx.create_password_user("gatekeeper").await;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let node = start(&url, |_| {}).await;
        let http = reqwest::Client::new();
        let res = http
            .post(format!("{}/api", node.origin))
            .json(&json!({
                "query": LOGIN,
                "variables": { "input": { "username": "gatekeeper", "password": PASSWORD } },
            }))
            .send()
            .await
            .expect("login");
        let cookie = |name: &str| {
            res.headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(|v| v.strip_prefix(&format!("{name}=")))
                .and_then(|v| v.split(';').next())
                .map(str::to_string)
                .unwrap_or_else(|| panic!("{name} cookie"))
        };
        let (session, csrf) = (cookie("rim_session"), cookie("rim_csrf"));
        Self {
            node,
            http,
            session,
            csrf,
        }
    }

    /// Posts `query` with the session cookie, and with the CSRF cookie and header when `csrf`.
    async fn post(&self, query: &str, csrf: bool) -> Value {
        let mut request = self.http.post(format!("{}/api", self.node.origin));
        request = if csrf {
            request
                .header(
                    "cookie",
                    format!("rim_session={}; rim_csrf={}", self.session, self.csrf),
                )
                .header("x-csrf-token", &self.csrf)
        } else {
            request.header("cookie", format!("rim_session={}", self.session))
        };
        request
            .json(&json!({ "query": query }))
            .send()
            .await
            .expect("post")
            .json()
            .await
            .expect("json")
    }

    async fn status_text(&self) -> Value {
        let res = self.post(STATUS_TEXT, false).await;
        assert!(res.get("errors").is_none(), "{res}");
        res["data"]["me"]["statusText"].clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_mutation_in_nested_fragments_needs_the_csrf_header() {
    let signed_in = SignedIn::new().await;

    let refused = signed_in.post(NESTED_PROTECTED, false).await;
    assert_eq!(
        refused["errors"][0]["message"], "csrf validation failed",
        "{refused}"
    );
    assert!(refused["data"].is_null(), "{refused}");
    assert_eq!(signed_in.status_text().await, Value::Null);

    // With the header the same document runs, so the refusal above was the check's.
    let ran = signed_in.post(NESTED_PROTECTED, true).await;
    assert!(ran.get("errors").is_none(), "{ran}");
    assert_eq!(signed_in.status_text().await, "changed without CSRF");

    signed_in.node.stop().await;
}
