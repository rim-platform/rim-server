//! "Keep me signed in" against a running server: the cookies and keys a signup gets with and
//! without it, the session secret an unkept session requires over HTTP and in the WebSocket's
//! `connection_init`, the keep key a kept web session reads back, and the reload window in
//! which a reloaded tab takes its reload key once.
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

use support::nodes::{Node, databases_with, start};
use support::relay::{RESET, eventually, key_signup_raw, keys, open_registration};
use support::ws::Socket;

const KEEP_KEY: &str = "query { sessionKeepKey }";
const SET_RELOAD_KEY: &str = "mutation { setReloadKey }";
const RELOAD: &str = "mutation { reloadSession { reloadKey nextReloadKey } }";
const LOGOUT: &str = "mutation { logout }";

/// The close code and reason of a connection whose first message lacks the session secret.
const SECRET_REQUIRED: (u16, &str) = (4401, "session secret required");
/// How long a refused connection may take to send its close frame.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// A signed-up account's session as its client holds it.
struct Signed {
    /// The `Set-Cookie` headers of the signup, attributes included.
    set_cookies: Vec<String>,
    user_id: String,
    token: String,
    csrf: String,
    session_secret: Option<String>,
    keep_key: Option<String>,
}

async fn node() -> Node {
    let (url, _) = databases_with(RESET).await;
    start(&url, open_registration).await
}

/// Sign `name` up on `node` with the seed's key, choosing `keep_signed_in` and `keep_key_held`.
async fn sign_up(
    node: &Node,
    name: &str,
    seed: u8,
    keep_signed_in: bool,
    keep_key_held: bool,
) -> Signed {
    let http = reqwest::Client::new();
    let (body, set_cookies) = key_signup_raw(
        &http,
        node,
        name,
        &keys(seed),
        json!({ "keepSignedIn": keep_signed_in, "keepKeyHeld": keep_key_held }),
    )
    .await;
    assert!(body.get("errors").is_none(), "signup: {body}");
    let data = &body["data"]["signupWithKey"];
    let cookie = |name: &str| {
        set_cookies
            .iter()
            .find_map(|c| c.strip_prefix(&format!("{name}=")))
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("no {name} cookie"))
            .to_string()
    };
    Signed {
        user_id: data["userId"].as_str().expect("userId").to_string(),
        token: cookie("rim_session"),
        csrf: cookie("rim_csrf"),
        session_secret: data["sessionSecret"].as_str().map(str::to_string),
        keep_key: data["keepKey"].as_str().map(str::to_string),
        set_cookies,
    }
}

/// POST `query` to `node` as `signed`, with `secret` in the session secret header when given.
/// Returns the whole answer.
async fn post(node: &Node, signed: &Signed, query: &str, secret: Option<&str>) -> Value {
    let mut request = reqwest::Client::new()
        .post(format!("{}/api", node.origin))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", signed.token, signed.csrf),
        )
        .header("x-csrf-token", &signed.csrf)
        .json(&json!({ "query": query }));
    if let Some(secret) = secret {
        request = request.header("x-rim-session-secret", secret);
    }
    request
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json")
}

/// The data of an answer without errors.
fn data(body: Value) -> Value {
    assert!(body.get("errors").is_none(), "{body}");
    body["data"].clone()
}

/// The code of an answer's first error.
fn code(body: &Value) -> &str {
    body["errors"][0]["extensions"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {body}"))
}

/// When the session of `signed` last closed a WebSocket, as stored.
async fn ws_closed_at(node: &Node, signed: &Signed) -> Option<chrono::DateTime<chrono::Utc>> {
    let row = node
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT ws_closed_at FROM session WHERE token_hash = $1",
            [rim_auth::session_token::hash_token(&signed.token).into()],
        ))
        .await
        .expect("query")
        .expect("the session");
    row.try_get("", "ws_closed_at").expect("ws_closed_at")
}

/// Open a WebSocket as `signed`, sending `secret` in `connection_init`, and wait for the ack.
async fn open_socket(node: &Node, signed: &Signed, secret: &str) -> Socket {
    let mut socket = Socket::upgrade(&node.origin, &signed.token)
        .await
        .expect("upgrade");
    socket
        .send(json!({ "type": "connection_init", "payload": { "sessionSecret": secret } }))
        .await;
    assert_eq!(socket.receive().await["type"], "connection_ack");
    socket
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cookies_and_keys_of_a_signup_follow_keep_me_signed_in() {
    let node = node().await;

    let unkept = sign_up(&node, "una", 31, false, false).await;
    assert_eq!(unkept.set_cookies.len(), 2);
    for cookie in &unkept.set_cookies {
        assert!(
            !cookie.contains("Max-Age"),
            "a browser-session cookie: {cookie}"
        );
    }
    assert!(unkept.keep_key.is_none());
    let secret = unkept.session_secret.as_deref().expect("a session secret");
    assert_eq!(
        data(post(&node, &unkept, KEEP_KEY, Some(secret)).await)["sessionKeepKey"],
        Value::Null
    );

    let web = sign_up(&node, "kai", 32, true, false).await;
    for cookie in &web.set_cookies {
        assert!(cookie.contains("Max-Age="), "a persistent cookie: {cookie}");
    }
    assert!(web.session_secret.is_none());
    let keep_key = web.keep_key.as_deref().expect("a keep key");
    assert_eq!(
        data(post(&node, &web, KEEP_KEY, None).await)["sessionKeepKey"],
        json!(keep_key)
    );

    let desktop = sign_up(&node, "dev", 33, true, true).await;
    for cookie in &desktop.set_cookies {
        assert!(cookie.contains("Max-Age="), "a persistent cookie: {cookie}");
    }
    assert!(desktop.session_secret.is_none() && desktop.keep_key.is_none());
    assert_eq!(
        data(post(&node, &desktop, KEEP_KEY, None).await)["sessionKeepKey"],
        Value::Null
    );
    // A kept session has no reload window.
    assert_eq!(
        code(&post(&node, &desktop, SET_RELOAD_KEY, None).await),
        "UNAUTHENTICATED"
    );
    assert_eq!(
        code(&post(&node, &desktop, RELOAD, None).await),
        "UNAUTHENTICATED"
    );

    // Signing out ends the session and the keep key with it.
    assert_eq!(data(post(&node, &web, LOGOUT, None).await)["logout"], true);
    assert_eq!(
        code(&post(&node, &web, KEEP_KEY, None).await),
        "UNAUTHENTICATED"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unkept_session_answers_only_requests_that_carry_its_secret() {
    let node = node().await;
    let unkept = sign_up(&node, "una", 34, false, false).await;
    let secret = unkept.session_secret.clone().expect("a session secret");

    assert_eq!(
        code(&post(&node, &unkept, KEEP_KEY, None).await),
        "UNAUTHENTICATED"
    );
    assert_eq!(
        code(&post(&node, &unkept, KEEP_KEY, Some("wrong")).await),
        "UNAUTHENTICATED"
    );
    let other = sign_up(&node, "oto", 35, false, false).await;
    assert_eq!(
        code(&post(&node, &unkept, KEEP_KEY, other.session_secret.as_deref()).await),
        "UNAUTHENTICATED",
        "another session's secret"
    );
    assert_eq!(
        data(post(&node, &unkept, "query { me { id } }", Some(&secret)).await)["me"]["id"],
        json!(unkept.user_id)
    );

    // Signing out needs no secret, and ends the session.
    assert_eq!(
        data(post(&node, &unkept, LOGOUT, None).await)["logout"],
        true
    );
    assert_eq!(
        code(&post(&node, &unkept, KEEP_KEY, Some(&secret)).await),
        "UNAUTHENTICATED"
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unkept_sessions_websocket_opens_only_with_its_secret_in_connection_init() {
    let node = node().await;
    let unkept = sign_up(&node, "una", 36, false, false).await;
    let secret = unkept.session_secret.clone().expect("a session secret");

    for init in [
        json!({ "type": "connection_init" }),
        json!({ "type": "connection_init", "payload": { "sessionSecret": "wrong" } }),
        json!({ "type": "ping", "payload": { "sessionSecret": secret } }),
    ] {
        let mut socket = Socket::upgrade(&node.origin, &unkept.token)
            .await
            .expect("upgrade");
        socket.send(init).await;
        let (code, reason) = SECRET_REQUIRED;
        socket.expect_close(code, reason, CLOSE_WAIT).await;
    }
    drop(open_socket(&node, &unkept, &secret).await);

    // A kept session's connection needs no secret.
    let kept = sign_up(&node, "kai", 37, true, true).await;
    drop(Socket::connect(&node.origin, &kept.token).await);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_takes_the_reload_key_once_within_a_minute_of_the_websocket_closing() {
    let node = node().await;
    let unkept = sign_up(&node, "una", 38, false, false).await;
    let secret = unkept.session_secret.clone().expect("a session secret");

    let reload_key =
        data(post(&node, &unkept, SET_RELOAD_KEY, Some(&secret)).await)["setReloadKey"]
            .as_str()
            .expect("a reload key")
            .to_string();
    // No WebSocket closed yet.
    assert_eq!(
        code(&post(&node, &unkept, RELOAD, None).await),
        "UNAUTHENTICATED"
    );

    // While a WebSocket is open the window stays closed, even after an earlier one closed.
    drop(open_socket(&node, &unkept, &secret).await);
    eventually("the first WebSocket's closing", || async {
        ws_closed_at(&node, &unkept).await.map(|_| ())
    })
    .await;
    let socket = open_socket(&node, &unkept, &secret).await;
    eventually("the second WebSocket's opening", || async {
        ws_closed_at(&node, &unkept).await.is_none().then_some(())
    })
    .await;
    assert_eq!(
        code(&post(&node, &unkept, RELOAD, None).await),
        "UNAUTHENTICATED"
    );

    // Once it closes, the tab takes the key once, without the secret.
    drop(socket);
    eventually("the second WebSocket's closing", || async {
        ws_closed_at(&node, &unkept).await.map(|_| ())
    })
    .await;
    let keys = data(post(&node, &unkept, RELOAD, None).await)["reloadSession"].clone();
    assert_eq!(keys["reloadKey"], json!(reload_key));
    let next = keys["nextReloadKey"].as_str().expect("the next key");
    assert_ne!(next, reload_key);
    assert_eq!(
        code(&post(&node, &unkept, RELOAD, None).await),
        "UNAUTHENTICATED",
        "taken once"
    );

    // After a minute the window is closed.
    drop(open_socket(&node, &unkept, &secret).await);
    eventually("the third WebSocket's closing", || async {
        ws_closed_at(&node, &unkept).await.map(|_| ())
    })
    .await;
    node.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE session SET ws_closed_at = now() - interval '61 seconds' \
             WHERE token_hash = $1",
            [rim_auth::session_token::hash_token(&unkept.token).into()],
        ))
        .await
        .expect("age the closing");
    assert_eq!(
        code(&post(&node, &unkept, RELOAD, None).await),
        "UNAUTHENTICATED"
    );

    // Signing out ends the reload key with the session.
    drop(open_socket(&node, &unkept, &secret).await);
    eventually("the fourth WebSocket's closing", || async {
        ws_closed_at(&node, &unkept).await.map(|_| ())
    })
    .await;
    assert_eq!(
        data(post(&node, &unkept, LOGOUT, None).await)["logout"],
        true
    );
    assert_eq!(
        code(&post(&node, &unkept, RELOAD, None).await),
        "UNAUTHENTICATED"
    );
    node.stop().await;
}
