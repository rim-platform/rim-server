//! The checks that classify a GraphQL operation, on a real server: a protected mutation hidden
//! in nested fragments still needs the CSRF header, and `/api/ws` runs subscriptions only.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use rim_api::broadcast::{Broadcaster, InMemoryBroadcaster, WorkspaceEvent};
use rim_core::server::model::Server;
use rim_core::user::model::User;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};

use support::nodes::{Node, S2S_TABLES, start};
use support::password::{LOGIN_WITH_PASSWORD, TEST_PASSWORD, login_over_http};
use support::relay::{key_signup, keys, login_proof, open_registration, statement};
use support::ws::Socket;

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

/// What a request carries of the CSRF token besides the session cookie.
enum Csrf {
    None,
    /// The `X-CSRF-Token` header without the `rim_csrf` cookie.
    HeaderOnly,
    CookieAndHeader,
}

/// A server, and a user signed up and signed in to it over HTTP, with a key and a password, who
/// owns one workspace.
struct SignedIn {
    node: Node,
    http: reqwest::Client,
    session: String,
    csrf: String,
    user: User,
    workspace: Server,
}

impl SignedIn {
    async fn new() -> Self {
        let ctx = support::TestCtx::new().await;
        ctx.conn
            .execute_unprepared(S2S_TABLES)
            .await
            .expect("truncate");
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let node = start(&url, open_registration).await;
        let http = reqwest::Client::new();
        let keys = keys(1);
        let (signup, _) = key_signup(&http, &node, "gatekeeper", &keys, json!({})).await;
        assert!(signup.get("errors").is_none(), "{signup}");
        let user = SeaOrmUserRepository::new(ctx.db.clone())
            .find_by_username("gatekeeper")
            .await
            .expect("find user")
            .expect("the signed-up user");
        let workspace = ctx.create_server("gate").await;
        ctx.add_owner(&workspace, &user).await;
        // The password login, then the key login with its ticket.
        let password =
            login_over_http(&http, &node.origin, &keys.state.principal_id, TEST_PASSWORD)
                .await
                .expect("the password logs in");
        let proof = login_proof(&http, &node, &keys).await;
        let res = http
            .post(format!("{}/api", node.origin))
            .json(&json!({
                "query": LOGIN_WITH_PASSWORD,
                "variables": password.variables(statement(&proof)),
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
        let body: Value = res.json().await.expect("json");
        assert_eq!(
            body["data"]["loginWithKey"]["csrfToken"],
            json!(csrf),
            "{body}"
        );
        Self {
            node,
            http,
            session,
            csrf,
            user,
            workspace,
        }
    }

    /// Posts `query` with the session cookie and what `csrf` adds.
    async fn post(&self, query: &str, csrf: Csrf) -> Value {
        let mut request = self.http.post(format!("{}/api", self.node.origin));
        request = match csrf {
            Csrf::None => request.header("cookie", format!("rim_session={}", self.session)),
            Csrf::HeaderOnly => request
                .header("cookie", format!("rim_session={}", self.session))
                .header("x-csrf-token", &self.csrf),
            Csrf::CookieAndHeader => request
                .header(
                    "cookie",
                    format!("rim_session={}; rim_csrf={}", self.session, self.csrf),
                )
                .header("x-csrf-token", &self.csrf),
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
        let res = self.post(STATUS_TEXT, Csrf::None).await;
        assert!(res.get("errors").is_none(), "{res}");
        res["data"]["me"]["statusText"].clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_protected_mutation_in_nested_fragments_needs_the_csrf_header() {
    let signed_in = SignedIn::new().await;

    // Without the header, and with the header but without the cookie it must equal.
    for csrf in [Csrf::None, Csrf::HeaderOnly] {
        let refused = signed_in.post(NESTED_PROTECTED, csrf).await;
        assert_eq!(
            refused["errors"][0]["message"], "csrf validation failed",
            "{refused}"
        );
        assert!(refused["data"].is_null(), "{refused}");
    }
    assert_eq!(signed_in.status_text().await, Value::Null);

    // With the token `loginWithKey` returned in the header and the cookie the same document runs, so
    // the refusals above were the check's.
    let ran = signed_in
        .post(NESTED_PROTECTED, Csrf::CookieAndHeader)
        .await;
    assert!(ran.get("errors").is_none(), "{ran}");
    assert_eq!(signed_in.status_text().await, "changed without CSRF");

    signed_in.node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_refuses_queries_and_mutations_and_runs_subscriptions() {
    let signed_in = SignedIn::new().await;
    let mut socket = Socket::connect(&signed_in.node.origin, &signed_in.session).await;

    for (id, query) in [
        (
            "mutation",
            r#"mutation { updateMyProfile(input: { statusText: "changed over WS" }) { id } }"#,
        ),
        ("query", "{ me { id } }"),
    ] {
        socket.subscribe(id, query, json!({})).await;
        let answer = socket.receive().await;
        assert_eq!(answer["type"], "next", "{answer}");
        assert_eq!(answer["id"], id, "{answer}");
        let errors = answer["payload"]["errors"].as_array().expect("errors");
        assert_eq!(errors.len(), 1, "{answer}");
        assert_eq!(
            errors[0]["message"],
            "only subscriptions over the WebSocket"
        );
        assert_eq!(errors[0]["extensions"]["code"], "VALIDATION");
        assert!(answer["payload"]["data"].is_null(), "{answer}");
        assert_eq!(
            socket.receive().await,
            json!({ "type": "complete", "id": id })
        );
    }
    assert_eq!(signed_in.status_text().await, Value::Null);

    socket
        .subscribe(
            "events",
            "subscription ($id: ID!) { workspaceEvents(workspaceId: $id) }",
            json!({ "id": signed_in.workspace.id.to_string() }),
        )
        .await;
    let broadcaster = signed_in
        .node
        .schema
        .data::<InMemoryBroadcaster>()
        .expect("broadcaster");
    // The subscription starts listening some time after `subscribe`; broadcast until it hears.
    let marker = "socket-gate-marker";
    let mut event = None;
    for _ in 0..50 {
        broadcaster.broadcast(WorkspaceEvent::PresenceChanged {
            workspace_id: signed_in.workspace.id,
            user_id: signed_in.user.id,
            status: marker.to_string(),
        });
        let Some(answer) = socket.receive_within(Duration::from_millis(200)).await else {
            continue;
        };
        assert_eq!(answer["type"], "next", "{answer}");
        assert_eq!(answer["id"], "events", "{answer}");
        assert!(answer["payload"].get("errors").is_none(), "{answer}");
        let text = answer["payload"]["data"]["workspaceEvents"]
            .as_str()
            .expect("an event");
        if text.contains(marker) {
            event = Some(text.to_string());
            break;
        }
    }
    let event = event.expect("the subscription delivers the event");
    assert!(event.contains("PresenceChanged"), "{event}");

    signed_in.node.stop().await;
}
