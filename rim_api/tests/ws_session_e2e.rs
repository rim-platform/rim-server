//! A WebSocket connection ends with its session: on a revocation in this process, on a
//! recheck that finds the session expired, and before a new operation on a session ended
//! where this process did not hear of it. Each closes with `4401` and sends nothing after.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_server::config::RIMConfig;
use support::nodes::{Node, S2S_TABLES, start};
use support::ws::Socket;

const WORKSPACE_EVENTS: &str = "subscription($w: ID!) { workspaceEvents(workspaceId: $w) }";
const SET_STATE: &str = "mutation($w: ID!, $v: JSON!) { setWorkspaceState(input: { \
     serverId: $w, key: \"ping\", value: $v, scope: SERVER }) { key } }";
const LOGOUT_OTHER_SESSIONS: &str = "mutation { logoutOtherSessions }";

/// The close code and reason of a connection whose session ended.
const SESSION_ENDED: (u16, &str) = (4401, "session ended");
/// How long a closed connection may take to send its close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// A node with `tweak` applied, where `ada` owns workspace `workspace`.
struct World {
    node: Node,
    ada: rim_core::user::model::UserId,
    workspace: String,
}

async fn world(tweak: impl FnOnce(&mut RIMConfig)) -> World {
    let ctx = support::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let ada = ctx.create_user("ada").await;
    let workspace = ctx.create_server("Ada's").await;
    ctx.add_owner(&workspace, &ada).await;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    World {
        node: start(&url, tweak).await,
        ada: ada.id,
        workspace: workspace.id.to_string(),
    }
}

/// A signed-in session's cookies.
struct Session {
    token: String,
    csrf: String,
}

impl World {
    /// A new session of `ada`, stored as a sign-in stores it, with its cookies.
    async fn sign_in(&self) -> Session {
        let token = rim_auth::session_token::generate_session_token();
        SeaOrmSessionRepository::new(self.node.db.clone().into())
            .create_session(NewSession {
                user_id: self.ada,
                token_hash: rim_auth::session_token::hash_token(&token),
                expires_at: chrono::Utc::now() + chrono::Duration::days(1),
                user_agent: None,
                ip: None,
            })
            .await
            .expect("session");
        Session {
            csrf: rim_auth::csrf::generate_csrf_token(&token),
            token,
        }
    }

    /// Run `query` over HTTP as `session` and return its data.
    async fn post(&self, session: &Session, query: &str, variables: Value) -> Value {
        let body: Value = reqwest::Client::new()
            .post(format!("{}/api", self.node.origin))
            .header(
                "cookie",
                format!("rim_session={}; rim_csrf={}", session.token, session.csrf),
            )
            .header("x-csrf-token", &session.csrf)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .expect("post")
            .json()
            .await
            .expect("json");
        assert!(body.get("errors").is_none(), "{body}");
        body["data"].clone()
    }

    /// Set a workspace state entry as `session`, which tells the workspace each time.
    async fn publish(&self, session: &Session, n: usize) {
        self.post(session, SET_STATE, json!({ "w": self.workspace, "v": n }))
            .await;
    }

    async fn open(&self, session: &Session) -> Result<Socket, u16> {
        Socket::open(&self.node.origin, &session.token).await
    }

    /// Subscribe to the workspace's events on `socket` and wait until the subscription
    /// delivers one: a workspace state change by `other`, one per attempt. Events of earlier
    /// attempts that arrive late are drained.
    async fn subscribe_live(&self, socket: &mut Socket, other: &Session) {
        socket
            .subscribe("1", WORKSPACE_EVENTS, json!({ "w": self.workspace }))
            .await;
        // The subscription may start after an event it should see; retry until it sees one.
        for n in 0..50 {
            self.publish(other, n).await;
            let mut delivered = false;
            while let Some(message) = socket.receive_within(Duration::from_millis(300)).await {
                assert_eq!(message["type"], "next", "{message}");
                assert_eq!(message["id"], "1");
                delivered = true;
            }
            if delivered {
                return;
            }
        }
        panic!("the subscription delivered no event");
    }
}

async fn expect_session_ended(socket: &mut Socket) {
    let (code, reason) = SESSION_ENDED;
    socket.expect_close(code, reason, CLOSE_WAIT).await;
}

/// Signing the other sessions out closes their open socket with `4401` and nothing after,
/// with the recheck off; the connection's slot is free again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_other_sessions_out_closes_their_websocket() {
    let w = world(|config| {
        config.rate_ws_connections_per_user = Some(1);
        config.ws_session_recheck_secs = Some(0);
    })
    .await;
    let desktop = w.sign_in().await;
    let laptop = w.sign_in().await;
    let mut socket = w.open(&desktop).await.expect("open");
    assert_eq!(
        w.open(&laptop).await.err(),
        Some(429),
        "one connection per user"
    );
    w.subscribe_live(&mut socket, &laptop).await;

    let data = w.post(&laptop, LOGOUT_OTHER_SESSIONS, json!({})).await;
    assert_eq!(data["logoutOtherSessions"], json!(1));
    // An event published now no longer reaches the closed socket.
    w.publish(&laptop, 100).await;
    expect_session_ended(&mut socket).await;

    // The slot is given back once the closed connection is gone.
    let mut reopened = None;
    for _ in 0..50 {
        match w.open(&laptop).await {
            Ok(socket) => {
                reopened = Some(socket);
                break;
            }
            Err(429) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(status) => panic!("upgrade refused with {status}"),
        }
    }
    assert!(reopened.is_some(), "the closed connection's slot is free");
    w.node.stop().await;
}

/// A session expired in the database closes its socket at the next recheck.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_recheck_closes_the_websocket_of_an_expired_session() {
    let w = world(|config| config.ws_session_recheck_secs = Some(1)).await;
    let desktop = w.sign_in().await;
    let laptop = w.sign_in().await;
    let mut socket = w.open(&desktop).await.expect("open");
    w.subscribe_live(&mut socket, &laptop).await;

    let token_hash = rim_auth::session_token::hash_token(&desktop.token);
    w.node
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE session SET expires_at = now() - interval '1 second' WHERE token_hash = $1",
            [token_hash.into()],
        ))
        .await
        .expect("expire the session");
    expect_session_ended(&mut socket).await;
    w.node.stop().await;
}

/// A session revoked where this process did not hear of it, with the recheck off: the next
/// operation is not answered and the socket closes. A query, which a live session would have
/// refused as not a subscription, shows the session check comes first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_operation_on_a_revoked_session_closes_the_websocket() {
    let w = world(|config| config.ws_session_recheck_secs = Some(0)).await;
    let desktop = w.sign_in().await;
    let mut socket = w.open(&desktop).await.expect("open");

    w.node
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE session SET revoked_at = now() WHERE user_id = $1",
            [uuid::Uuid::from(w.ada).into()],
        ))
        .await
        .expect("revoke the session");
    socket.subscribe("1", "{ me { id } }", json!({})).await;
    expect_session_ended(&mut socket).await;
    w.node.stop().await;
}
