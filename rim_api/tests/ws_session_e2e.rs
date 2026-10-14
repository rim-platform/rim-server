//! A WebSocket connection ends with its session: on a revocation in this process, on a
//! recheck that finds the session expired, and before a new operation on a session ended
//! where this process did not hear of it. Each closes with `4401` and sends nothing after.
//! A client message over the size cap ends the connection the same way with `1009`.
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
use rim_core::user::model::{NewUser, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
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
/// The largest client message the socket takes.
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
/// The close code and reason of a connection whose client sent a message over the cap.
const TOO_LARGE: (u16, &str) = (1009, "message too large");
/// The fragment size the size tests send in.
const FRAGMENT: usize = 4096;

/// A node with `tweak` applied, where `ada` owns workspace `workspace`.
struct World {
    node: Node,
    ada: UserId,
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
        self.sign_in_as(self.ada).await
    }

    /// A new session of `user_id`, stored as a sign-in stores it, with its cookies.
    async fn sign_in_as(&self, user_id: UserId) -> Session {
        let token = rim_auth::session_token::generate_session_token();
        SeaOrmSessionRepository::new(self.node.db.clone().into())
            .create_session(NewSession {
                user_id,
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

    /// Open a connection as `session`, waiting while a closed connection still holds the
    /// user's slot.
    async fn reopen(&self, session: &Session) -> Socket {
        for _ in 0..50 {
            match self.open(session).await {
                Ok(socket) => return socket,
                Err(429) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(status) => panic!("upgrade refused with {status}"),
            }
        }
        panic!("the closed connection's slot is not free");
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

async fn expect_too_large(socket: &mut Socket) {
    let (code, reason) = TOO_LARGE;
    socket.expect_close(code, reason, CLOSE_WAIT).await;
}

/// A `graphql-transport-ws` ping of exactly `len` bytes, padded in its payload.
fn ping_of_len(len: usize) -> Vec<u8> {
    let empty = json!({ "type": "ping", "payload": { "pad": "" } }).to_string();
    let pad = "x".repeat(len - empty.len());
    let ping = json!({ "type": "ping", "payload": { "pad": pad } }).to_string();
    assert_eq!(ping.len(), len);
    ping.into_bytes()
}

/// `socket` answers a ping sent in fragments with a pong.
async fn expect_pong_to_fragmented_ping(socket: &mut Socket, len: usize) {
    socket.send_fragmented(&ping_of_len(len), FRAGMENT).await;
    assert_eq!(
        socket.receive().await["type"],
        "pong",
        "a ping of {len} bytes"
    );
}

/// Fragmented messages just under and right at the cap reach the engine, which answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fragmented_messages_up_to_the_size_cap_are_answered() {
    let w = world(|_| {}).await;
    let session = w.sign_in().await;
    let mut socket = w.open(&session).await.expect("open");

    for len in [MAX_MESSAGE_BYTES - 1, MAX_MESSAGE_BYTES] {
        expect_pong_to_fragmented_ping(&mut socket, len).await;
    }
    w.node.stop().await;
}

/// A fragmented message one byte over the cap closes the socket with `1009` and nothing
/// after; the connection's slot is free again and a new connection works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fragmented_message_over_the_size_cap_closes_the_websocket() {
    let w = world(|config| config.rate_ws_connections_per_user = Some(1)).await;
    let session = w.sign_in().await;
    let mut socket = w.open(&session).await.expect("open");

    socket
        .send_fragmented(&ping_of_len(MAX_MESSAGE_BYTES + 1), FRAGMENT)
        .await;
    expect_too_large(&mut socket).await;

    let mut reopened = w.reopen(&session).await;
    expect_pong_to_fragmented_ping(&mut reopened, FRAGMENT * 2).await;
    w.node.stop().await;
}

/// A frame announcing more than the cap closes the socket with `1009` at its header, without
/// waiting for the payload.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_over_the_size_cap_closes_the_websocket_at_its_header() {
    let w = world(|_| {}).await;
    let session = w.sign_in().await;
    let mut socket = w.open(&session).await.expect("open");

    socket.send_frame_header(MAX_MESSAGE_BYTES as u64 + 1).await;
    expect_too_large(&mut socket).await;
    w.node.stop().await;
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
    w.reopen(&laptop).await;
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

/// An instance operator blocking an account in the app closes the account's open socket at
/// once, with the recheck off.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_blocking_an_account_in_the_app_closes_its_websocket() {
    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    let w = world(|config| config.ws_session_recheck_secs = Some(0)).await;
    w.node
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE "user" SET principal_id = $1, identity_key = 'ed25519:e2e' WHERE id = $2"#,
            [PRINCIPAL.into(), uuid::Uuid::from(w.ada).into()],
        ))
        .await
        .expect("hold a principal");
    let opal = SeaOrmUserRepository::new(w.node.db.clone().into())
        .create_user(NewUser {
            username: "opal".to_string(),
            password: Some("$argon2id$v=invalid$test_only".to_string()),
            email: Some("opal@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("operator account");
    let config = RIMConfig {
        database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL"),
        ..Default::default()
    };
    let granted = rim_server::operator_cmd::run_with(
        &config,
        ["grant", "opal"].into_iter().map(str::to_string),
    )
    .await
    .expect("grant");
    assert_eq!(granted, "opal is now an instance operator\n");

    let desktop = w.sign_in().await;
    let mut socket = w.open(&desktop).await.expect("open");
    socket
        .subscribe("1", WORKSPACE_EVENTS, json!({ "w": w.workspace }))
        .await;
    let operator = w.sign_in_as(opal.id).await;
    let data = w
        .post(
            &operator,
            "mutation($p: String!) { instanceBlockPrincipal(principalId: $p, reason: \"spam\") \
             { endedSessions } }",
            json!({ "p": PRINCIPAL }),
        )
        .await;
    assert_eq!(data["instanceBlockPrincipal"][0]["endedSessions"], json!(1));
    expect_session_ended(&mut socket).await;
    w.node.stop().await;
}
