//! Two RIM servers in one process, each with its own database (`DATABASE_URL` and the same name
//! with `b` appended, created when missing), talking over plain HTTP on loopback with
//! `RIM_S2S_ALLOW_HTTP` and `RIM_S2S_ALLOW_PRIVATE_ADDRESSES`: discovery, pinning, signed
//! requests and responses, and every refusal of the transport.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use migration::MigratorTrait;
use rim_identity::SignedStatement;
use rim_infra_services::s2s::server_key;
use rim_s2s::http_sig::{
    IncomingResponse, KeyId, OutgoingRequest, RequestBinding, new_nonce, parse_response,
    sign_request,
};
use rim_s2s::{
    ServerIdentity, ServerKeyState, ServerRotationSigner, ServerSigner, sign_inception,
    sign_rotation, verify_descriptor,
};
use rim_server::app::S2sRuntime;
use rim_server::config::RIMConfig;
use rim_services::s2s::dto::{S2sMethod, S2sRequest};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;

/// S2S tables, cleared before each test so every server boots a fresh identity.
const S2S_TABLES: &str = "TRUNCATE TABLE server_key_log_entry, server_descriptor, peer_key_log_entry, peer_server, \
     s2s_nonce";

/// A running server: its origin, data directory, database and S2S handles.
struct Node {
    origin: String,
    data: tempfile::TempDir,
    db: DatabaseConnection,
    s2s: S2sRuntime,
    handle: actix_web::dev::ServerHandle,
    _shutdown: tokio::sync::watch::Sender<bool>,
}

impl Node {
    fn identity(&self) -> Arc<ServerIdentity> {
        self.s2s.state.identity.current()
    }

    fn id(&self) -> String {
        self.identity().server_id().to_string()
    }

    async fn stop(self) {
        self.handle.stop(false).await;
    }
}

/// The second test database: the first one's name with `b` appended, created when missing.
async fn second_database(first: &DatabaseConnection) -> String {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base.to_string(), format!("?{query}")),
        None => (url.clone(), String::new()),
    };
    let (prefix, name) = base.rsplit_once('/').expect("database name");
    let second = format!("{name}b");
    let exists = first
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM pg_database WHERE datname = $1",
            [second.clone().into()],
        ))
        .await
        .expect("pg_database")
        .is_some();
    if !exists {
        first
            .execute_unprepared(&format!("CREATE DATABASE \"{second}\""))
            .await
            .expect("create the second test database");
    }
    format!("{prefix}/{second}{query}")
}

/// Both test databases, migrated, with their S2S tables empty.
async fn databases() -> (String, String) {
    let ctx = support::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let first = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let second = second_database(&ctx.conn).await;
    let conn = Database::connect(&second).await.expect("second database");
    migration::Migrator::up(&conn, None).await.expect("migrate");
    conn.execute_unprepared(S2S_TABLES).await.expect("truncate");
    (first, second)
}

async fn start(database_url: &str, tweak: impl FnOnce(&mut RIMConfig)) -> Node {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let origin = format!("http://127.0.0.1:{port}");
    let data = tempfile::tempdir().expect("data dir");
    let mut config = RIMConfig {
        server_host: "127.0.0.1".into(),
        server_port: port,
        database_url: database_url.into(),
        data_dir: data.path().to_string_lossy().into_owned(),
        public_base_url: Some(origin.clone()),
        s2s_allow_http: Some(true),
        s2s_allow_private_addresses: Some(true),
        ..Default::default()
    };
    tweak(&mut config);
    let running = rim_server::server::serve(&config, listener)
        .await
        .expect("serve");
    let handle = running.server.handle();
    tokio::spawn(running.server);
    Node {
        origin,
        data,
        db: Database::connect(database_url).await.expect("db"),
        s2s: running.s2s,
        handle,
        _shutdown: running.shutdown_tx,
    }
}

async fn two_nodes() -> (Node, Node) {
    let (first, second) = databases().await;
    let a = start(&first, |_| {}).await;
    let b = start(&second, |_| {}).await;
    (a, b)
}

/// A request signed by hand, so tests can replay, tamper with and age it.
struct Manual {
    method: &'static str,
    target_uri: String,
    body: Option<Vec<u8>>,
    nonce: String,
    created: i64,
    expires: i64,
}

impl Manual {
    fn get(to: &Node, path: &str) -> Self {
        let created = Utc::now().timestamp();
        Self {
            method: "GET",
            target_uri: format!("{}{path}", to.origin),
            body: None,
            nonce: new_nonce().expect("nonce"),
            created,
            expires: created + 300,
        }
    }

    fn post(to: &Node, path: &str, body: &[u8]) -> Self {
        Self {
            method: "POST",
            body: Some(body.to_vec()),
            ..Self::get(to, path)
        }
    }

    /// Sign as `signer` at `key_id` from `origin`, and send `sent_body` (the signed body unless
    /// given).
    async fn send_as(
        &self,
        signer: &ServerSigner,
        key_id: &KeyId,
        origin: &str,
        sent_body: Option<&[u8]>,
    ) -> (reqwest::Response, Option<String>) {
        let headers = sign_request(
            signer,
            &OutgoingRequest {
                method: self.method,
                target_uri: &self.target_uri,
                body: self.body.as_deref(),
                content_type: "application/json",
                rim_origin: origin,
                created: self.created,
                expires: self.expires,
                nonce: &self.nonce,
                key_id,
            },
        )
        .expect("sign");
        let client = reqwest::Client::new();
        let mut request = match self.method {
            "POST" => client
                .post(&self.target_uri)
                .header("content-type", "application/json")
                .body(
                    sent_body
                        .or(self.body.as_deref())
                        .unwrap_or_default()
                        .to_vec(),
                ),
            _ => client.get(&self.target_uri),
        }
        .header("rim-origin", origin)
        .header("signature-input", &headers.signature_input)
        .header("signature", &headers.signature);
        if let Some(digest) = &headers.content_digest {
            request = request.header("content-digest", digest);
        }
        (request.send().await.expect("send"), headers.content_digest)
    }

    async fn send(&self, from: &Node) -> (reqwest::Response, Option<String>) {
        let identity = from.identity();
        self.send_as(&identity.signer, &identity.key_id(), &from.origin, None)
            .await
    }

    /// Check the response's signature by `to`'s current key and return status and body.
    async fn answer(&self, to: &Node, sent: (reqwest::Response, Option<String>)) -> (u16, Value) {
        let (response, digest) = sent;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let (cd, ct, nonce, input, sig) = (
            header("content-digest"),
            header("content-type"),
            header("rim-request-nonce"),
            header("signature-input"),
            header("signature"),
        );
        let body = response.bytes().await.expect("body");
        let pending = parse_response(
            &IncomingResponse {
                status,
                content_digest: cd.as_deref(),
                content_type: ct.as_deref(),
                rim_request_nonce: nonce.as_deref(),
                signature_input: input.as_deref(),
                signature: sig.as_deref(),
            },
            &RequestBinding {
                method: self.method,
                target_uri: &self.target_uri,
                content_digest: digest.as_deref(),
                nonce: &self.nonce,
            },
            Utc::now().timestamp(),
        )
        .expect("the response is signed for this request");
        assert_eq!(pending.key_id, to.identity().key_id());
        pending.check_digest(&body).expect("digest");
        pending
            .verify(&to.identity().state.server_key)
            .expect("signature");
        (status, serde_json::from_slice(&body).expect("json"))
    }
}

fn ping() -> S2sRequest {
    S2sRequest {
        peer_server_id: String::new(),
        method: S2sMethod::Get,
        path: "/ping".into(),
        body: vec![],
        content_type: None,
    }
}

async fn client_ping(from: &Node, to: &Node) -> rim_services::s2s::dto::S2sResponse {
    from.s2s
        .client
        .send(S2sRequest {
            peer_server_id: to.id(),
            ..ping()
        })
        .await
        .expect("a verified answer")
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_document_is_signed_and_cacheable_from_any_origin() {
    let (first, second) = databases().await;
    let a = start(&first, |_| {}).await;

    let response = reqwest::Client::new()
        .get(format!("{}/.well-known/rim-server", a.origin))
        .header("origin", "https://elsewhere.example")
        .send()
        .await
        .expect("discovery");
    assert_eq!(response.status(), 200);
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    };
    assert_eq!(header("cache-control").as_deref(), Some("max-age=300"));
    assert_eq!(header("access-control-allow-origin").as_deref(), Some("*"));
    assert!(
        header("content-type")
            .unwrap_or_default()
            .starts_with("application/json")
    );
    let doc: Value = response.json().await.expect("json");
    assert_eq!(doc["rim"], "server-discovery");
    let statement = |v: &Value| SignedStatement {
        payload: v["payload"].as_str().expect("payload").to_string(),
        signature: v["signature"].as_str().expect("signature").to_string(),
    };
    let log: Vec<SignedStatement> = doc["keyLog"]
        .as_array()
        .expect("log")
        .iter()
        .map(statement)
        .collect();
    let state = ServerKeyState::replay(&log).expect("log replays");
    assert_eq!(state.server_id, a.id());
    let descriptor = verify_descriptor(&state, &statement(&doc["descriptor"])).expect("descriptor");
    assert_eq!(descriptor.version, 1);
    assert_eq!(descriptor.key_seq, 0);
    assert_eq!(
        descriptor.roles,
        vec![rim_s2s::Role::Host, rim_s2s::Role::Relay]
    );
    assert_eq!(descriptor.endpoints.graphql, format!("{}/api", a.origin));
    assert_eq!(
        descriptor.endpoints.websocket,
        format!("ws://{}/api/ws", a.origin.trim_start_matches("http://"))
    );
    assert_eq!(
        descriptor.endpoints.s2s.as_deref(),
        Some(format!("{}/s2s/v1", a.origin).as_str())
    );
    assert_eq!(descriptor.mls_cipher_suites, vec![79]);
    let key_file = a.data.path().join("server-key.json");
    assert!(key_file.exists());
    assert!(a.data.path().join("server-recovery-key.txt").exists());
    a.stop().await;

    // With server-to-server traffic off, s2s is null and /s2s/v1 is not served.
    let off = start(&second, |c| c.s2s_allow_http = Some(false)).await;
    let doc: Value = reqwest::get(format!("{}/.well-known/rim-server", off.origin))
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json");
    let payload: Value =
        serde_json::from_str(doc["descriptor"]["payload"].as_str().expect("payload"))
            .expect("json");
    assert_eq!(payload["endpoints"]["s2s"], Value::Null);
    let response = reqwest::get(format!("{}/s2s/v1/ping", off.origin))
        .await
        .expect("ping");
    assert_eq!(response.status(), 404);
    off.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_pins_a_and_a_signed_ping_round_trips_both_ways() {
    let (a, b) = two_nodes().await;
    let pinned = b
        .s2s
        .state
        .peers
        .pin(&a.origin, Some(&a.id()))
        .await
        .expect("pin");
    assert_eq!(pinned.server_id, a.id());
    assert_eq!(pinned.status, "active");

    let answer = client_ping(&b, &a).await;
    assert_eq!(answer.status, 200);
    assert_eq!(answer.key_seq, 0);
    let body: Value = serde_json::from_slice(&answer.body).expect("json");
    assert_eq!(body["serverId"], a.id());
    assert_eq!(body["roles"], serde_json::json!(["host", "relay"]));
    // A pinned B from its Rim-Origin while verifying the request.
    let b_at_a = a
        .s2s
        .state
        .peers
        .get(&b.id())
        .await
        .expect("get")
        .expect("pinned");
    assert_eq!(b_at_a.origin, b.origin);

    let back = client_ping(&a, &b).await;
    assert_eq!(back.status, 200);
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replayed_tampered_expired_unknown_and_misrouted_requests_are_refused() {
    let (a, b) = two_nodes().await;

    let request = Manual::get(&a, "/s2s/v1/ping");
    let (status, body) = request.answer(&a, request.send(&b).await).await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = request.answer(&a, request.send(&b).await).await;
    assert_eq!((status, error_code(&body)), (409, "replay"));

    let post = Manual::post(&a, "/s2s/v1/ping", br#"{"a":1}"#);
    let identity = b.identity();
    let sent = post
        .send_as(
            &identity.signer,
            &identity.key_id(),
            &b.origin,
            Some(br#"{"a":2}"#),
        )
        .await;
    let (status, body) = post.answer(&a, sent).await;
    assert_eq!((status, error_code(&body)), (401, "bad_signature"));

    let mut expired = Manual::get(&a, "/s2s/v1/ping");
    expired.created -= 600;
    expired.expires -= 600;
    let (status, body) = expired.answer(&a, expired.send(&b).await).await;
    assert_eq!((status, error_code(&body)), (401, "bad_signature"));

    // A server nobody can pin: its Rim-Origin serves another server's log.
    let stranger = ServerSigner::from_seed([77; 32]);
    let inception = sign_inception(&stranger, &ServerSigner::from_seed([78; 32]), Utc::now());
    let stranger_id = KeyId {
        server_id: rim_s2s::server_id_for_inception(&inception.payload),
        seq: 0,
    };
    let unknown = Manual::get(&a, "/s2s/v1/ping");
    let (status, body) = unknown
        .answer(
            &a,
            unknown
                .send_as(&stranger, &stranger_id, &b.origin, None)
                .await,
        )
        .await;
    assert_eq!((status, error_code(&body)), (403, "unknown_peer"));

    // A key the pin does not name.
    let wrong_key = Manual::get(&a, "/s2s/v1/ping");
    let (status, body) = wrong_key
        .answer(
            &a,
            wrong_key
                .send_as(&stranger, &b.identity().key_id(), &b.origin, None)
                .await,
        )
        .await;
    assert_eq!((status, error_code(&body)), (401, "bad_signature"));

    let missing = Manual::post(&a, "/s2s/v1/nothing", b"{}");
    let (status, body) = missing.answer(&a, missing.send(&b).await).await;
    assert_eq!((status, error_code(&body)), (404, "not_found"));

    // Too large, before anything else is looked at.
    let big = vec![b' '; rim_server::config::DEFAULT_S2S_MAX_BODY_BYTES + 1];
    let too_large = Manual::post(&a, "/s2s/v1/ping", &big);
    let (status, body) = too_large.answer(&a, too_large.send(&b).await).await;
    assert_eq!((status, error_code(&body)), (413, "too_large"));
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_peer_is_refused() {
    let (first, second) = databases().await;
    let b = start(&second, |_| {}).await;
    let denied = b.id();
    let a = start(&first, |c| c.s2s_denied_peers = Some(denied)).await;
    b.s2s.state.peers.pin(&a.origin, None).await.expect("pin");
    let answer = client_ping(&b, &a).await;
    assert_eq!(answer.status, 403);
    let body: Value = serde_json::from_slice(&answer.body).expect("json");
    assert_eq!(error_code(&body), "peer_not_allowed");
    assert!(a.s2s.state.peers.get(&b.id()).await.expect("get").is_none());
    a.stop().await;
    b.stop().await;
}

async fn set_key_file(path: &Path, signer: &ServerSigner) {
    use base64::Engine;
    let _ = std::fs::remove_file(path);
    let text = format!(
        r#"{{"v":1,"seed":"{}"}}"#,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signer.seed())
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .expect("key file")
        .write_all(text.as_bytes())
        .expect("write");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotated_key_has_a_grace_period_and_a_forked_log_is_refused_until_reset() {
    let (a, b) = two_nodes().await;
    // Pin both ways explicitly, so A's one refetch a minute for B is still unused.
    b.s2s
        .state
        .peers
        .pin(&a.origin, Some(&a.id()))
        .await
        .expect("pin a");
    a.s2s
        .state
        .peers
        .pin(&b.origin, Some(&b.id()))
        .await
        .expect("pin b");
    assert_eq!(client_ping(&b, &a).await.status, 200);

    let before = b.identity();
    server_key::rotate(b.s2s.identity_repo.as_ref(), &b.s2s.key_files)
        .await
        .expect("rotate");
    assert!(b.s2s.reload_identity().await.expect("reload"));
    assert_eq!(b.identity().state.seq, 1);
    assert_eq!(b.identity().descriptor_content.key_seq, 1);
    assert_eq!(b.identity().descriptor_content.version, 2);

    // Signed at seq 1: A refetches B's log, sees the rotation and accepts it.
    let answer = client_ping(&b, &a).await;
    assert_eq!(answer.status, 200);
    let b_at_a = a
        .s2s
        .state
        .peers
        .get(&b.id())
        .await
        .expect("get")
        .expect("pinned");
    assert_eq!(b_at_a.head_seq, 1);

    // The previous key still works during the grace period…
    let old = Manual::get(&a, "/s2s/v1/ping");
    let sent = old
        .send_as(&before.signer, &before.key_id(), &b.origin, None)
        .await;
    assert_eq!(old.answer(&a, sent).await.0, 200);
    // …and not once 24 hours have passed since A learned of the rotation.
    a.db.execute_unprepared(&format!(
        "UPDATE peer_server SET rotated_at = now() - interval '25 hours' WHERE server_id = '{}'",
        b.id()
    ))
    .await
    .expect("age the rotation");
    let old = Manual::get(&a, "/s2s/v1/ping");
    let sent = old
        .send_as(&before.signer, &before.key_id(), &b.origin, None)
        .await;
    let (status, body) = old.answer(&a, sent).await;
    assert_eq!((status, error_code(&body)), (401, "bad_signature"));

    // B's history is rewritten: another seq 1 and a seq 2, signed by the seq 0 key.
    let inception = SignedStatement {
        payload: before.log[0].payload.clone(),
        signature: before.log[0].signature.clone(),
    };
    let state0 = ServerKeyState::from_inception(&inception).expect("inception");
    let k1 = ServerSigner::from_seed([91; 32]);
    let k2 = ServerSigner::from_seed([92; 32]);
    let e1 = sign_rotation(
        &state0,
        &before.signer,
        ServerRotationSigner::Server,
        &k1.public_key(),
        &state0.recovery_key,
        Utc::now(),
    );
    let state1 = state0.apply(&e1).expect("e1");
    let e2 = sign_rotation(
        &state1,
        &k1,
        ServerRotationSigner::Server,
        &k2.public_key(),
        &state1.recovery_key,
        Utc::now(),
    );
    b.db.execute_unprepared("DELETE FROM server_key_log_entry WHERE seq > 0")
        .await
        .expect("drop seq 1");
    for (seq, entry) in [(1, &e1), (2, &e2)] {
        b.db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO server_key_log_entry (seq, entry_type, payload, signature) \
             VALUES ($1, 'rotate', $2, $3)",
            [
                (seq as i32).into(),
                entry.payload.clone().into(),
                entry.signature.clone().into(),
            ],
        ))
        .await
        .expect("insert");
    }
    set_key_file(&b.s2s.key_files.key_file, &k2).await;
    assert!(b.s2s.reload_identity().await.expect("reload"));
    assert_eq!(b.identity().state.seq, 2);

    // A's one refetch a minute for B was spent on the rotation; an explicit refresh finds the
    // fork.
    assert!(
        a.s2s
            .state
            .peers
            .pin(&b.origin, Some(&b.id()))
            .await
            .is_err()
    );
    let answer = client_ping(&b, &a).await;
    assert_eq!(answer.status, 403);
    let body: Value = serde_json::from_slice(&answer.body).expect("json");
    assert_eq!(error_code(&body), "peer_forked");
    let b_at_a = a
        .s2s
        .state
        .peers
        .get(&b.id())
        .await
        .expect("get")
        .expect("pinned");
    assert_eq!(b_at_a.status, "forked");

    // After an out-of-band check the operator resets the pin and B is pinned afresh.
    assert!(a.s2s.state.peers.reset(&b.id()).await.expect("reset"));
    a.s2s
        .state
        .peers
        .pin(&b.origin, Some(&b.id()))
        .await
        .expect("pin again");
    assert_eq!(client_ping(&b, &a).await.status, 200);
    a.stop().await;
    b.stop().await;
}
