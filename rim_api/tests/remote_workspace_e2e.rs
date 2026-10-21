//! Two RIM servers, one hosting a workspace and one the relay of a person who joins it: the join
//! through the relay with channel traffic both ways and every host event checked against the
//! host's key, presence, a kick; workspace items queued for subscribers only; a delegation
//! expiring and renewed; the host's allow-list; a
//! device revocation acknowledged through the relay and brought directly to a host the relay
//! cannot reach; a recovery at the relay applied by the host, ending the old devices' direct
//! sessions and delegations there; the direct sign-in of a remote member with its day-long
//! session; join and login challenges kept apart; `leaveRelay`; and the SDL.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rim_api::auth_context::AuthActorCtx;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use rim_identity::testing::{
    SigningKey, TestIdentity, device_delegation, device_login, signing_key,
};
use rim_identity::{
    Delegation, KeyState, PublicKey, Purpose, SignedStatement, StatementVersion,
    UnverifiedRevocation, device_request_payload, sign_with_context,
};
use rim_s2s::host::{HostEventFields, sign_host_event, verify_host_event, verify_host_response};
use rim_s2s::revocation::verify_revocation_ack;
use rim_server::config::RIMConfig;
use rim_services::relay::dto::{DeliverItem, SignedJson};
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::{WorkspaceHostService, WorkspaceRelayService};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use support::nodes::{Node, databases_with, start};
use support::unknown_id;

/// Everything these tests touch, emptied on both databases first.
const RESET: &str = "TRUNCATE TABLE server_key_log_entry, server_descriptor, peer_key_log_entry, \
     peer_server, peer_server_forgotten, s2s_nonce, s2s_outbox, s2s_peer_delivery, s2s_peer_inbound, \
     relay_envelope, \
     remote_identity_cache, key_challenge, host_delegation, relay_subscription, \
     relay_revocation_fanout, \"user\", server RESTART IDENTITY CASCADE";

const SIGNUP: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId csrfToken } }"#;
const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";
const LOGIN: &str = r#"
mutation Login($proof: SignedStatementInput!) { loginWithKey(proof: $proof) { userId csrfToken } }"#;
const PUBLISH: &str = r#"
mutation Publish($card: SignedStatementInput!) { publishProfileCard(card: $card) { version } }"#;
const CREATE_SERVER: &str =
    r#"mutation Create($input: CreateServerInput!) { createServer(input: $input) { id } }"#;
const CREATE_INVITE: &str = r#"
mutation Invite($input: CreateInviteInput!) { createInvite(input: $input) { inviteToken } }"#;
const SEND: &str = r#"
mutation Send($input: SendMessageInput!) { sendMessage(input: $input) { id } }"#;
const KICK: &str = r#"
mutation Kick($s: ID!, $u: ID!) { kickMember(serverId: $s, userId: $u) }"#;
const PRESENCE: &str = r#"
query Presence($w: ID!) { workspacePresence(workspaceId: $w) { userId status } }"#;
const HEADS: &str = r#"
query Heads($s: ID!, $streams: [String!]!) { streamHeads(serverId: $s, streams: $streams) { stream seq } }"#;
const REMOTE_CHALLENGE: &str = r#"
mutation Challenge($h: String!, $o: String!, $t: String) {
  remoteJoinChallenge(hostServerId: $h, hostOrigin: $o, inviteToken: $t) {
    challengeId nonce expiresAt workspaceId
  }
}"#;
const JOIN: &str = r#"
mutation Join($input: JoinRemoteWorkspaceInput!) {
  joinRemoteWorkspace(input: $input) { hostServerId workspaceId userId role }
}"#;
const RENEW: &str = r#"
mutation Renew($d: [SignedStatementInput!]!) {
  renewDelegations(delegations: $d) { hostServerId workspaceId status expiresAt reason }
}"#;
const SUBSCRIPTIONS: &str =
    "query { remoteSubscriptions { hostServerId workspaceId status expiresAt reason } }";
const ADD_DEVICE: &str = r#"
mutation Add($c: SignedStatementInput!) { addDeviceKey(certificate: $c) { deviceId } }"#;
const REVOKE: &str = r#"
mutation Revoke($r: SignedStatementInput!) { revokeDeviceKey(revocation: $r) { revokedAt } }"#;
const REVOCATION_STATUS: &str = r#"
query Status($d: ID!) {
  deviceRevocationStatus(deviceId: $d) { hostServerId status attempts ack { payload signature } }
}"#;
const APPLY_REVOCATION: &str = r#"
mutation Apply($r: SignedStatementInput!) { applyRemoteRevocation(revocation: $r) { payload signature } }"#;
const ROTATE: &str = r#"
mutation Rotate($input: RotateIdentityKeyInput!) { rotateIdentityKey(input: $input) { keySeq } }"#;
const LEAVE: &str = "mutation { leaveRelay }";
const ITEMS: &str = r#"
query Items($after: ID) {
  relayItems(after: $after, limit: 500) {
    items { id hostServerId workspaceId envelope { payload signature } } hasMore
  }
}"#;

fn open_registration(config: &mut RIMConfig) {
    config.open_registration = Some(true);
}

/// A host and a relay, both open for registration.
async fn nodes() -> (Node, Node) {
    let (h, r) = databases_with(RESET).await;
    (
        start(&h, open_registration).await,
        start(&r, open_registration).await,
    )
}

fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

fn signed(v: &Value) -> SignedStatement {
    SignedStatement {
        payload: v["payload"].as_str().expect("payload").to_string(),
        signature: v["signature"].as_str().expect("signature").to_string(),
    }
}

fn host_key(node: &Node) -> PublicKey {
    node.identity().state.server_key.clone()
}

/// Poll `check` until it gives a value, for up to 20 seconds.
async fn eventually<T, F, Fut>(what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for _ in 0..100 {
        if let Some(value) = check().await {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn scalar(node: &Node, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    node.db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n")
}

async fn user_id(node: &Node, principal_id: &str) -> UserId {
    let row = node
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id FROM "user" WHERE principal_id = $1"#,
            [principal_id.into()],
        ))
        .await
        .expect("query")
        .expect("user row");
    row.try_get::<Uuid>("", "id").expect("id").into()
}

async fn device_key_id(node: &Node, device_id: Uuid) -> DeviceKeyId {
    let row = node
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM device_key WHERE device_id = $1",
            [device_id.into()],
        ))
        .await
        .expect("query")
        .expect("device row");
    row.try_get::<Uuid>("", "id").expect("id").into()
}

/// Active delegations a host holds for a principal.
async fn active_delegations(host: &Node, principal_id: &str) -> i64 {
    scalar(
        host,
        r#"SELECT count(*) AS n FROM host_delegation d JOIN "user" u ON u.id = d.user_id
           WHERE u.principal_id = $1 AND d.ended_at IS NULL AND d.expires_at > now()"#,
        vec![principal_id.into()],
    )
    .await
}

/// A direct sign-in at a host that succeeded.
struct DirectSignIn {
    http: reqwest::Client,
    session: String,
    csrf: String,
    /// The `loginWithKey` answer.
    login: Value,
}

/// Sign in to `host` directly with a device of the identity at `state`; the answer when it is
/// refused.
async fn sign_in_directly(
    host: &Node,
    device_id: Uuid,
    device_key: &SigningKey,
    state: &KeyState,
) -> Result<DirectSignIn, Value> {
    let http = reqwest::Client::new();
    let challenge: Value = http
        .post(format!("{}/api", host.origin))
        .json(&json!({ "query": CHALLENGE }))
        .send()
        .await
        .expect("challenge")
        .json()
        .await
        .expect("json");
    let challenge = &challenge["data"]["keyLoginChallenge"];
    let proof = device_login(
        device_key,
        state,
        device_id,
        challenge["challengeId"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        &host.id(),
    );
    let response = http
        .post(format!("{}/api", host.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login");
    let has_session = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().is_ok_and(|t| t.starts_with("rim_session=")));
    if !has_session {
        return Err(response.json().await.expect("json"));
    }
    let (session, csrf) = cookies(&response);
    let body: Value = response.json().await.expect("json");
    if body.get("errors").is_some() {
        return Err(body);
    }
    Ok(DirectSignIn {
        http,
        session,
        csrf,
        login: body["data"]["loginWithKey"].clone(),
    })
}

/// A queued item whose envelope verified under its host's key.
#[derive(Debug, Clone)]
struct Item {
    stream: String,
    seq: Option<u64>,
    workspace_id: String,
    event: Value,
}

fn verify_item(host: &Node, item: &Value) -> Item {
    assert_eq!(item["hostServerId"], json!(host.id()));
    let event = verify_host_event(&host_key(host), &signed(&item["envelope"]))
        .expect("a host-signed event");
    assert_eq!(event.host, host.id());
    assert_eq!(json!(event.workspace_id), item["workspaceId"]);
    if let Some(conversation) = event.event.get("conversation_id").and_then(Value::as_str)
        && event.stream.starts_with("c:")
    {
        assert_eq!(event.stream, format!("c:{conversation}"));
    }
    Item {
        stream: event.stream,
        seq: event.seq,
        workspace_id: event.workspace_id,
        event: event.event,
    }
}

/// An answer to a forwarded request, after its host response was checked.
struct Forwarded {
    status: u16,
    body: Value,
}

/// A person with one device, signed up on a server over HTTP.
struct Person {
    origin: String,
    http: reqwest::Client,
    session: String,
    csrf: String,
    device_id: Uuid,
    device_key: SigningKey,
    identity: TestIdentity,
    state: KeyState,
    seq: u64,
}

/// The session and CSRF cookies a response set.
fn cookies(response: &reqwest::Response) -> (String, String) {
    let mut found = std::collections::HashMap::new();
    for value in response.headers().get_all("set-cookie") {
        let text = value.to_str().expect("cookie");
        if let Some((name, rest)) = text.split_once('=') {
            found.insert(
                name.to_string(),
                rest.split(';').next().unwrap_or_default().to_string(),
            );
        }
    }
    (
        found.remove("rim_session").expect("session cookie"),
        found.remove("rim_csrf").expect("csrf cookie"),
    )
}

async fn sign_up(node: &Node, name: &str, seed: u8) -> Person {
    let http = reqwest::Client::new();
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let inception = identity.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::from_u128(0x2000 + u128::from(seed));
    let device_key = signing_key(seed.wrapping_add(60));
    let cert = identity.device_cert(&state, device_id, &device_key, "Desktop");
    let challenge: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": CHALLENGE }))
        .send()
        .await
        .expect("challenge")
        .json()
        .await
        .expect("json");
    let challenge = &challenge["data"]["keyLoginChallenge"];
    let proof = device_login(
        &device_key,
        &state,
        device_id,
        challenge["challengeId"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        &node.id(),
    );
    let response = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": SIGNUP, "variables": { "input": {
            "username": name,
            "keyLog": [statement(&inception)],
            "device": statement(&cert),
            "proof": statement(&proof),
        } } }))
        .send()
        .await
        .expect("signup");
    let (session, csrf) = cookies(&response);
    let body: Value = response.json().await.expect("json");
    assert!(body.get("errors").is_none(), "signup: {body}");
    assert_eq!(body["data"]["signupWithKey"]["csrfToken"], json!(csrf));
    let person = Person {
        origin: node.origin.clone(),
        http,
        session,
        csrf,
        device_id,
        device_key,
        identity,
        state,
        seq: 1_791_504_000_000_000 + u64::from(seed) * 1_000_000_000,
    };
    let card = person.identity.profile_card(
        &person.state,
        1,
        name,
        &[node.origin.as_str()],
        &[(device_id, &person.device_key)],
    );
    person
        .ok(PUBLISH, json!({ "card": statement(&card) }))
        .await;
    person
}

impl Person {
    fn principal(&self) -> String {
        self.state.principal_id.clone()
    }

    async fn post_to(&self, origin: &str, query: &str, variables: Value) -> Value {
        self.http
            .post(format!("{origin}/api"))
            .header(
                "cookie",
                format!("rim_session={}; rim_csrf={}", self.session, self.csrf),
            )
            .header("x-csrf-token", &self.csrf)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .expect("send")
            .json()
            .await
            .expect("json")
    }

    async fn ok(&self, query: &str, variables: Value) -> Value {
        let body = self.post_to(&self.origin, query, variables).await;
        assert!(body.get("errors").is_none(), "errors: {body}");
        body["data"].clone()
    }

    async fn err(&self, query: &str, variables: Value) -> String {
        let body = self.post_to(&self.origin, query, variables).await;
        body["errors"][0]["extensions"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {body}"))
            .to_string()
    }

    /// A delegation of this device for `relay` to carry `workspace` on `host`, from a minute
    /// ago for `hours`.
    fn delegation(
        &self,
        host: &Node,
        relay: &Node,
        workspace: &str,
        hours: i64,
    ) -> SignedStatement {
        let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now");
        device_delegation(
            &self.device_key,
            &Delegation {
                principal_id: self.principal(),
                device_id: self.device_id,
                relay: relay.id(),
                relay_key_hash: rim_s2s::key_hash(&relay.identity().state.server_key),
                host: host.id(),
                workspace_id: workspace.parse().expect("uuid"),
                nonce: URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes()),
                issued_at: now - chrono::Duration::minutes(1),
                expires_at: now - chrono::Duration::minutes(1) + chrono::Duration::hours(hours),
            },
        )
    }

    /// Join `workspace` on `host` through this person's relay `relay`. With an invite the
    /// workspace comes from the challenge, as an app learns it.
    async fn join(
        &self,
        host: &Node,
        relay: &Node,
        workspace: &str,
        invite: Option<&str>,
    ) -> Value {
        let challenge = self
            .ok(
                REMOTE_CHALLENGE,
                json!({ "h": host.id(), "o": host.origin, "t": invite }),
            )
            .await["remoteJoinChallenge"]
            .clone();
        let workspace = match invite {
            Some(_) => {
                let named = challenge["workspaceId"]
                    .as_str()
                    .expect("the invite's workspace");
                assert_eq!(named, workspace);
                named
            }
            None => {
                assert!(challenge["workspaceId"].is_null());
                workspace
            }
        };
        let proof = device_login(
            &self.device_key,
            &self.state,
            self.device_id,
            challenge["challengeId"]
                .as_str()
                .expect("id")
                .parse()
                .expect("uuid"),
            challenge["nonce"].as_str().expect("nonce"),
            &host.id(),
        );
        let delegation = self.delegation(host, relay, workspace, 12);
        self.ok(
            JOIN,
            json!({ "input": {
                "hostServerId": host.id(),
                "hostOrigin": host.origin,
                "inviteToken": invite,
                "proof": statement(&proof),
                "delegation": statement(&delegation),
            } }),
        )
        .await["joinRemoteWorkspace"]
            .clone()
    }

    /// Send a device-signed request through the relay to `host`; checks the host response.
    async fn forward(&mut self, host: &Node, body: Value) -> Forwarded {
        self.seq += 1;
        let body = serde_json::to_vec(&body).expect("json");
        let payload = device_request_payload(
            &self.principal(),
            self.device_id,
            &host.id(),
            "POST",
            "/api",
            self.seq,
            &body,
            "2026-10-09T00:00:00Z",
        );
        let signature = sign_with_context(
            Purpose::DeviceRequest.context(),
            &self.device_key,
            payload.clone(),
        )
        .signature;
        let response = self
            .http
            .post(format!("{}/api/remote/{}/api", self.origin, host.id()))
            .header("content-type", "application/json")
            .header(
                "cookie",
                format!("rim_session={}; rim_csrf={}", self.session, self.csrf),
            )
            .header("x-csrf-token", &self.csrf)
            .header("rim-device-request", URL_SAFE_NO_PAD.encode(&payload))
            .header("rim-device-signature", signature)
            .body(body)
            .send()
            .await
            .expect("send");
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let (encoded, host_signature) = header("rim-host-response")
            .zip(header("rim-host-signature"))
            .expect("a host-signed answer");
        let bytes = response.bytes().await.expect("body");
        let host_response = SignedStatement {
            payload: String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).expect("b64"))
                .expect("utf8"),
            signature: host_signature,
        };
        let verified = verify_host_response(&host_key(host), &host_response).expect("verified");
        assert!(verified.check(&payload, status, &bytes));
        Forwarded {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// Run GraphQL at `host` through the relay; panics unless it succeeded.
    async fn remote(&mut self, host: &Node, query: &str, variables: Value) -> Value {
        let answer = self
            .forward(host, json!({ "query": query, "variables": variables }))
            .await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert!(answer.body.get("errors").is_none(), "{}", answer.body);
        answer.body["data"].clone()
    }

    /// The relay's queued items, each envelope verified under `host`'s key.
    async fn items(&self, host: &Node) -> Vec<Item> {
        let data = self.ok(ITEMS, json!({ "after": null })).await;
        data["relayItems"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| verify_item(host, item))
            .collect()
    }

    async fn item_of(&self, host: &Node, kind: &str, matches: impl Fn(&Item) -> bool) -> Item {
        eventually(kind, || async {
            self.items(host)
                .await
                .into_iter()
                .find(|i| i.event["type"] == kind && matches(i))
        })
        .await
    }

    /// Add a device certified by the identity key; returns its id and key.
    async fn add_device(&self, n: u8) -> (Uuid, SigningKey) {
        let device_id = Uuid::from_u128(0x3000 + u128::from(n));
        let key = signing_key(n);
        let cert = self
            .identity
            .device_cert(&self.state, device_id, &key, "Phone");
        self.ok(ADD_DEVICE, json!({ "c": statement(&cert) })).await;
        (device_id, key)
    }

    async fn revocation_status(&self, device_id: Uuid) -> Vec<Value> {
        self.ok(REVOCATION_STATUS, json!({ "d": device_id.to_string() }))
            .await["deviceRevocationStatus"]
            .as_array()
            .expect("statuses")
            .clone()
    }
}

/// Read a local user's relay stream on `node` in the background; each pushed JSON arrives on
/// the channel. Aborting the task closes the stream.
fn relay_stream(
    node: &Node,
    user: UserId,
    device: DeviceKeyId,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<Value>,
) {
    let mut stream = node.schema.execute_stream(
        Request::new("subscription { relayStream }")
            .data(AuthActorCtx::new(Some(user), None).with_device_key_id(Some(device))),
    );
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(response) = stream.next().await {
            let Ok(data) = response.data.into_json() else {
                continue;
            };
            if let Some(text) = data["relayStream"].as_str()
                && let Ok(pushed) = serde_json::from_str::<Value>(text)
                && sender.send(pushed).is_err()
            {
                return;
            }
        }
    });
    (task, receiver)
}

/// A workspace `owner` creates on `host`, its #general channel, and an invite to it.
async fn workspace(host: &Node, owner: &Person) -> (String, String, String) {
    let workspace = owner
        .ok(CREATE_SERVER, json!({ "input": { "name": "Hosted" } }))
        .await["createServer"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let general = host
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM conversation WHERE server_id = $1 AND title = 'general'",
            [workspace.parse::<Uuid>().expect("uuid").into()],
        ))
        .await
        .expect("query")
        .expect("general")
        .try_get::<Uuid>("", "id")
        .expect("id")
        .to_string();
    let invite = owner
        .ok(CREATE_INVITE, json!({ "input": { "serverId": workspace } }))
        .await["createInvite"]["inviteToken"]
        .as_str()
        .expect("token")
        .to_string();
    (workspace, general, invite)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_member_joins_through_the_relay_and_channel_traffic_flows_both_ways() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let mut rita = sign_up(&r, "rita", 2).await;
    let (ws, general, invite) = workspace(&h, &hannah).await;

    assert_eq!(
        rita.err(
            REMOTE_CHALLENGE,
            json!({ "h": h.id(), "o": h.origin, "t": "not-an-invite" })
        )
        .await,
        "NOT_FOUND"
    );

    let joined = rita.join(&h, &r, &ws, Some(&invite)).await;
    assert_eq!(joined["hostServerId"], json!(h.id()));
    assert_eq!(joined["workspaceId"], json!(ws));
    assert_eq!(joined["role"], "member");
    let rita_on_h = user_id(&h, &rita.principal()).await;
    assert_eq!(joined["userId"], json!(rita_on_h.to_string()));
    assert_eq!(
        scalar(
            &h,
            r#"SELECT count(*) AS n FROM "user" WHERE id = $1 AND kind = 'remote' AND home_relay = $2"#,
            vec![Uuid::from(rita_on_h).into(), r.id().into()],
        )
        .await,
        1,
        "the host holds Rita as a remote user whose relay presented her delegation"
    );
    assert_eq!(
        scalar(
            &h,
            r#"SELECT count(*) AS n FROM "user" WHERE id = $1 AND home_username = 'rita'
                 AND home_handle_domain = '127.0.0.1'"#,
            vec![Uuid::from(rita_on_h).into()],
        )
        .await,
        1,
        "the join carries Rita's username at her relay, whose handle domain is checked"
    );
    assert_eq!(active_delegations(&h, &rita.principal()).await, 1);
    let subscriptions = rita.ok(SUBSCRIPTIONS, json!({})).await["remoteSubscriptions"].clone();
    assert_eq!(subscriptions[0]["status"], "active");
    assert_eq!(subscriptions[0]["hostServerId"], json!(h.id()));
    assert!(subscriptions[0]["expiresAt"].is_string());
    // A current member joins again without an invite.
    assert_eq!(
        rita.join(&h, &r, &ws, None).await["role"],
        "member",
        "a current member joins without an invite"
    );

    // Rita writes through her relay; the host runs it as her device.
    let heads = rita
        .remote(
            &h,
            HEADS,
            json!({ "s": ws, "streams": ["w", format!("c:{general}")] }),
        )
        .await["streamHeads"]
        .clone();
    assert_eq!(heads.as_array().expect("heads").len(), 2);
    let sent = rita
        .remote(
            &h,
            SEND,
            json!({ "input": { "serverId": ws, "conversationId": general, "text": "hello from rita" } }),
        )
        .await["sendMessage"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) AS n FROM message WHERE id = $1 AND text = 'hello from rita'",
            vec![sent.parse::<Uuid>().expect("uuid").into()],
        )
        .await,
        1
    );

    // Hannah writes on the host; the event reaches Rita's relay signed by the host.
    hannah
        .ok(
            SEND,
            json!({ "input": { "serverId": ws, "conversationId": general, "text": "hello from hannah" } }),
        )
        .await;
    let theirs = rita
        .item_of(&h, "NewMessage", |i| {
            i.event["message"]["text"] == "hello from hannah"
        })
        .await;
    let mine = rita
        .item_of(&h, "NewMessage", |i| {
            i.event["message"]["id"] == json!(sent)
        })
        .await;
    assert_eq!(theirs.stream, format!("c:{general}"));
    assert_eq!(theirs.workspace_id, ws);
    assert_eq!(theirs.seq, mine.seq.map(|s| s + 1), "one stream, no gap");

    // While Rita's relay stream is open, the host sees her online.
    let rita_on_r = user_id(&r, &rita.principal()).await;
    let device_on_r = device_key_id(&r, rita.device_id).await;
    let (stream, _pushed) = relay_stream(&r, rita_on_r, device_on_r);
    eventually("Rita online at the host", || async {
        let presence = hannah.ok(PRESENCE, json!({ "w": ws })).await["workspacePresence"].clone();
        presence
            .as_array()?
            .iter()
            .any(|p| p["userId"] == json!(rita_on_h.to_string()) && p["status"] == "ONLINE")
            .then_some(())
    })
    .await;
    stream.abort();

    // A kick reaches Rita on her own stream, then her delegations end.
    hannah
        .ok(KICK, json!({ "s": ws, "u": rita_on_h.to_string() }))
        .await;
    let left = rita
        .item_of(&h, "MemberLeft", |i| {
            i.event["user_id"] == json!(rita_on_h.to_string())
        })
        .await;
    assert_eq!(left.stream, format!("u:{}", rita.principal()));
    eventually("the kicked member's delegations ended", || async {
        (active_delegations(&h, &rita.principal()).await == 0).then_some(())
    })
    .await;
    let refused = rita
        .forward(
            &h,
            json!({ "query": HEADS, "variables": { "s": ws, "streams": ["w"] } }),
        )
        .await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.body["error"], "not_delegated");

    h.stop().await;
    r.stop().await;
}

/// An item `host` signs for `to` with outbox id `id`: a `w` event of `workspace_id`.
fn workspace_item(host: &Node, id: i64, workspace_id: &str, to: &Person) -> DeliverItem {
    let identity = host.identity();
    let server_id = host.id();
    let issued_at = rim_s2s::timestamp(Utc::now());
    let event = json!({
        "type": "WorkspacePluginChanged",
        "workspace_id": workspace_id,
        "plugin_id": format!("plugin-{id}"),
        "change": "added",
    })
    .to_string();
    let statement = sign_host_event(
        &identity.signer,
        &HostEventFields {
            host: &server_id,
            key_seq: identity.state.seq,
            workspace_id,
            stream: "w",
            seq: Some(u64::try_from(id).expect("id")),
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    DeliverItem {
        id: id.to_string(),
        envelope: SignedJson {
            payload: statement.payload,
            signature: statement.signature,
        },
        recipients: vec![to.principal()],
        device_id: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_items_are_queued_for_subscribers_and_refused_otherwise() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;

    // Ids above anything the host's own sender delivered, so neither is skipped as seen.
    let elsewhere = unknown_id();
    let answer = r
        .s2s
        .relay
        .deliver(
            h.id(),
            vec![
                workspace_item(&h, 1_000_000, &ws, &rita),
                workspace_item(&h, 1_000_001, &elsewhere, &rita),
            ],
        )
        .await
        .expect("delivered");
    assert_eq!(answer.acked_through, "1000001");
    assert_eq!(answer.refused.len(), 1, "{:?}", answer.refused);
    assert_eq!(answer.refused[0].id, "1000001");
    assert_eq!(answer.refused[0].reason, "not_subscribed");
    assert_eq!(answer.refused[0].principal_id, Some(rita.principal()));

    let subscribed = rita
        .item_of(&h, "WorkspacePluginChanged", |i| {
            i.event["plugin_id"] == "plugin-1000000"
        })
        .await;
    assert_eq!(subscribed.workspace_id, ws);
    assert_eq!(subscribed.stream, "w");
    assert!(
        rita.items(&h)
            .await
            .iter()
            .all(|i| i.workspace_id != elsewhere),
        "nothing is queued for a workspace Rita holds no subscription to"
    );

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegation_expires_and_is_renewed() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let mut rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;
    let heads = json!({ "query": HEADS, "variables": { "s": ws, "streams": ["w"] } });
    assert_eq!(rita.forward(&h, heads.clone()).await.status, 200);

    // The host's clock ends the delegation; the relay's subscription lapses with it.
    h.db.execute_unprepared("UPDATE host_delegation SET expires_at = now() - interval '1 second'")
        .await
        .expect("expire");
    let expired = rita.forward(&h, heads.clone()).await;
    assert_eq!(expired.status, 403);
    assert_eq!(expired.body["error"], "not_delegated");
    assert!(
        h.s2s
            .workspace_host
            .cleanup()
            .await
            .expect("cleanup")
            .ended_delegations
            >= 1
    );
    r.db.execute_unprepared(
        "UPDATE relay_subscription SET expires_at = now() - interval '1 second'",
    )
    .await
    .expect("expire");
    assert_eq!(
        r.s2s
            .workspace_relay
            .cleanup()
            .await
            .expect("cleanup")
            .lapsed_subscriptions,
        1
    );
    let lapsed = rita.ok(SUBSCRIPTIONS, json!({})).await["remoteSubscriptions"][0].clone();
    assert_eq!(lapsed["status"], "lapsed");

    // A fresh delegation brings it back.
    let fresh = rita.delegation(&h, &r, &ws, 6);
    let renewed =
        rita.ok(RENEW, json!({ "d": [statement(&fresh)] })).await["renewDelegations"].clone();
    assert_eq!(renewed[0]["status"], "active", "{renewed}");
    assert_eq!(renewed[0]["workspaceId"], json!(ws));
    assert!(renewed[0]["reason"].is_null());
    assert_eq!(
        rita.ok(SUBSCRIPTIONS, json!({})).await["remoteSubscriptions"][0]["status"],
        "active"
    );
    assert_eq!(rita.forward(&h, heads).await.status, 200);

    // A workspace Rita is not a member of is refused by the host, with its reason.
    let elsewhere = rita.delegation(&h, &r, &unknown_id(), 6);
    let refused = rita
        .ok(RENEW, json!({ "d": [statement(&elsewhere)] }))
        .await["renewDelegations"][0]
        .clone();
    assert_eq!(refused["status"], "refused");
    assert_eq!(refused["reason"], "not_member");
    // One naming another relay never leaves this one.
    let mut other_relay = rita.delegation(&h, &h, &ws, 6);
    other_relay.signature.push('x');
    assert_eq!(
        rita.err(RENEW, json!({ "d": [statement(&other_relay)] }))
            .await,
        "VALIDATION"
    );

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hosts_allow_list_refuses_another_relay() {
    let (first, second) = databases_with(RESET).await;
    let h = start(&first, |c| {
        open_registration(c);
        c.host_allowed_relays =
            Some("rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c".to_string());
    })
    .await;
    let r = start(&second, open_registration).await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, _) = workspace(&h, &hannah).await;
    assert_eq!(
        rita.err(REMOTE_CHALLENGE, json!({ "h": h.id(), "o": h.origin }))
            .await,
        "FORBIDDEN"
    );
    assert_eq!(
        scalar(&h, "SELECT count(*) AS n FROM key_challenge", vec![]).await,
        1,
        "only Hannah's signup challenge: none for the refused relay"
    );
    let _ = ws;
    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revocation_is_acknowledged_and_brought_directly_to_a_host_the_relay_cannot_reach() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;
    let rita_on_h = user_id(&h, &rita.principal()).await;
    let host_devices = |n: i64| {
        let h = &h;
        async move {
            (scalar(
                h,
                "SELECT count(*) AS n FROM device_key WHERE user_id = $1",
                vec![Uuid::from(rita_on_h).into()],
            )
            .await
                == n)
                .then_some(())
        }
    };

    // A phone the host learns about from the relay, then revoked by the identity key.
    let (phone, _) = rita.add_device(71).await;
    eventually("the host to learn the phone", || host_devices(2)).await;
    let rita_on_r = user_id(&r, &rita.principal()).await;
    let device_on_r = device_key_id(&r, rita.device_id).await;
    let (stream, mut pushed) = relay_stream(&r, rita_on_r, device_on_r);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let revocation = rita.identity.revoke_device(&rita.state, phone);
    rita.ok(REVOKE, json!({ "r": statement(&revocation) }))
        .await;
    let acked = eventually("the host's acknowledgement", || async {
        rita.revocation_status(phone)
            .await
            .into_iter()
            .find(|s| s["status"] == "ACKED")
    })
    .await;
    assert_eq!(acked["hostServerId"], json!(h.id()));
    let ack = verify_revocation_ack(&host_key(&h), &signed(&acked["ack"])).expect("host-signed");
    assert!(ack.acknowledges(&revocation.payload));
    assert_eq!(ack.device_id, phone.to_string());
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) AS n FROM device_key WHERE device_id = $1 AND revoked_at IS NOT NULL",
            vec![phone.into()],
        )
        .await,
        1
    );
    let notice = loop {
        let next = tokio::time::timeout(Duration::from_secs(20), pushed.recv())
            .await
            .expect("a notice in time")
            .expect("the stream stays open");
        if next["type"] == "RevocationAcked" {
            break next;
        }
    };
    assert_eq!(notice["deviceId"], json!(phone.to_string()));
    assert_eq!(notice["hostServerId"], json!(h.id()));
    assert_eq!(notice["ack"], acked["ack"]);
    stream.abort();

    // The relay cannot reach the host: the revocation waits for a retry, and the app brings
    // it to the host directly.
    let (tablet, _) = rita.add_device(72).await;
    eventually("the host to learn the tablet", || host_devices(3)).await;
    r.db.execute_unprepared(&format!(
        "UPDATE peer_server SET status = 'blocked' WHERE server_id = '{}'",
        h.id()
    ))
    .await
    .expect("block");
    let revocation = rita.identity.revoke_device(&rita.state, tablet);
    rita.ok(REVOKE, json!({ "r": statement(&revocation) }))
        .await;
    let pending = eventually("a failed attempt", || async {
        rita.revocation_status(tablet)
            .await
            .into_iter()
            .find(|s| s["status"] == "PENDING" && s["attempts"].as_i64() >= Some(1))
    })
    .await;
    assert!(pending["ack"].is_null());
    let direct: Value = reqwest::Client::new()
        .post(format!("{}/api", h.origin))
        .json(&json!({ "query": APPLY_REVOCATION, "variables": { "r": statement(&revocation) } }))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    assert!(direct.get("errors").is_none(), "{direct}");
    let direct_ack = signed(&direct["data"]["applyRemoteRevocation"]);
    assert!(
        verify_revocation_ack(&host_key(&h), &direct_ack)
            .expect("host-signed")
            .acknowledges(&revocation.payload)
    );
    assert_eq!(
        UnverifiedRevocation::read(&revocation)
            .expect("read")
            .device_id,
        tablet
    );
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) AS n FROM device_key WHERE device_id = $1 AND revoked_at IS NOT NULL",
            vec![tablet.into()],
        )
        .await,
        1
    );

    // Once the host is reachable again the retry gets the same acknowledgement.
    r.db.execute_unprepared(&format!(
        "UPDATE peer_server SET status = 'active' WHERE server_id = '{}'",
        h.id()
    ))
    .await
    .expect("unblock");
    r.db.execute_unprepared("UPDATE relay_revocation_fanout SET next_attempt_at = now()")
        .await
        .expect("due");
    let retried = eventually("the retried acknowledgement", || async {
        rita.revocation_status(tablet)
            .await
            .into_iter()
            .find(|s| s["status"] == "ACKED")
    })
    .await;
    assert_eq!(
        signed(&retried["ack"]),
        direct_ack,
        "a repeat gets the same acknowledgement"
    );

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovery_at_the_relay_reaches_the_host_and_revokes_the_old_devices() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let mut rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;
    let rita_on_h = user_id(&h, &rita.principal()).await;
    let (phone, _) = rita.add_device(73).await;
    eventually("the host to learn the phone", || async {
        (scalar(
            &h,
            "SELECT count(*) AS n FROM device_key WHERE device_id = $1",
            vec![phone.into()],
        )
        .await
            == 1)
            .then_some(())
    })
    .await;

    // Every device lost: the recovery key signs a new identity key and a new laptop, without
    // a session.
    let recovery = rita.identity.recover(&rita.state, 74);
    rita.state = rita.state.apply(&recovery).expect("recovery").0;
    let laptop = Uuid::from_u128(0x3000 + 75);
    let cert = rita
        .identity
        .device_cert(&rita.state, laptop, &signing_key(75), "Laptop");
    let body: Value = reqwest::Client::new()
        .post(format!("{}/api", r.origin))
        .json(&json!({ "query": ROTATE, "variables": { "input": {
            "entry": statement(&recovery),
            "device": statement(&cert),
        } } }))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    assert!(body.get("errors").is_none(), "recovery: {body}");

    eventually("the host to apply the recovery", || async {
        (scalar(
            &h,
            "SELECT count(*) AS n FROM key_log_entry WHERE user_id = $1",
            vec![Uuid::from(rita_on_h).into()],
        )
        .await
            == 2)
            .then_some(())
    })
    .await;
    let unrevoked = |device_id: Uuid| {
        let h = &h;
        async move {
            scalar(
                h,
                "SELECT count(*) AS n FROM device_key \
                 WHERE user_id = $1 AND device_id = $2 AND revoked_at IS NULL",
                vec![Uuid::from(rita_on_h).into(), device_id.into()],
            )
            .await
        }
    };
    assert_eq!(unrevoked(rita.device_id).await, 0);
    assert_eq!(unrevoked(phone).await, 0);
    assert_eq!(unrevoked(laptop).await, 1);

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovery_ends_the_old_devices_direct_sessions_and_delegations_at_the_host() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let mut rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;
    assert_eq!(active_delegations(&h, &rita.principal()).await, 1);

    // The old device signs in to the host directly while the relay is up.
    let signed_in = sign_in_directly(&h, rita.device_id, &rita.device_key, &rita.state)
        .await
        .unwrap_or_else(|body| panic!("{body}"));
    let direct = Person {
        origin: h.origin.clone(),
        session: signed_in.session,
        csrf: signed_in.csrf,
        http: signed_in.http,
        device_id: rita.device_id,
        device_key: rita.device_key.clone(),
        identity: TestIdentity::with_version(2, StatementVersion::V2),
        state: rita.state.clone(),
        seq: 0,
    };
    direct.ok(HEADS, json!({ "s": ws, "streams": ["w"] })).await;

    // The device is lost: the recovery key signs a new identity key and a new laptop.
    let old_state = rita.state.clone();
    let recovery = rita.identity.recover(&rita.state, 76);
    rita.state = rita.state.apply(&recovery).expect("recovery").0;
    let laptop = Uuid::from_u128(0x3000 + 77);
    let cert = rita
        .identity
        .device_cert(&rita.state, laptop, &signing_key(77), "Laptop");
    let body: Value = reqwest::Client::new()
        .post(format!("{}/api", r.origin))
        .json(&json!({ "query": ROTATE, "variables": { "input": {
            "entry": statement(&recovery),
            "device": statement(&cert),
        } } }))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    assert!(body.get("errors").is_none(), "recovery: {body}");

    let rita_on_h = user_id(&h, &rita.principal()).await;
    eventually("the host to apply the recovery", || async {
        (scalar(
            &h,
            "SELECT count(*) AS n FROM key_log_entry WHERE user_id = $1",
            vec![Uuid::from(rita_on_h).into()],
        )
        .await
            == 2)
            .then_some(())
    })
    .await;

    assert_eq!(
        direct
            .err(HEADS, json!({ "s": ws, "streams": ["w"] }))
            .await,
        "UNAUTHENTICATED",
        "the old device's direct session ended"
    );
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) AS n FROM session WHERE user_id = $1 AND revoked_at IS NULL",
            vec![Uuid::from(rita_on_h).into()],
        )
        .await,
        0
    );
    assert_eq!(active_delegations(&h, &rita.principal()).await, 0);
    let refused = sign_in_directly(&h, rita.device_id, &rita.device_key, &old_state)
        .await
        .err()
        .expect("the old device is refused");
    assert!(refused.get("errors").is_some(), "{refused}");

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_member_signs_in_to_the_host_directly_for_a_day() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;

    // The relay is down; the app signs in to the host directly.
    r.stop().await;
    let signed_in = sign_in_directly(&h, rita.device_id, &rita.device_key, &rita.state)
        .await
        .unwrap_or_else(|body| panic!("{body}"));
    let rita_on_h = user_id(&h, &rita.principal()).await;
    assert_eq!(signed_in.login["userId"], json!(rita_on_h.to_string()));
    assert_eq!(signed_in.login["csrfToken"], json!(signed_in.csrf));
    let hours = scalar(
        &h,
        "SELECT (extract(epoch FROM (expires_at - now())) / 60)::bigint AS n FROM session \
         WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
        vec![Uuid::from(rita_on_h).into()],
    )
    .await;
    assert!((23 * 60..=24 * 60).contains(&hours), "{hours} minutes");

    let direct = Person {
        origin: h.origin.clone(),
        session: signed_in.session,
        csrf: signed_in.csrf,
        http: signed_in.http,
        device_id: rita.device_id,
        device_key: rita.device_key.clone(),
        identity: TestIdentity::with_version(2, StatementVersion::V2),
        state: rita.state.clone(),
        seq: 0,
    };
    let heads = direct.ok(HEADS, json!({ "s": ws, "streams": ["w"] })).await["streamHeads"].clone();
    assert_eq!(heads[0]["stream"], "w");
    let card = direct.identity.profile_card(
        &direct.state,
        5,
        "Rita",
        &[h.origin.as_str()],
        &[(rita.device_id, &rita.device_key)],
    );
    assert_eq!(
        direct
            .err(PUBLISH, json!({ "card": statement(&card) }))
            .await,
        "FORBIDDEN",
        "account mutations stay at the person's relay"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_proof_does_not_sign_in_at_the_host_nor_a_login_proof_join() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;

    // The app signs a proof over a join challenge the relay fetched; the relay presents it to
    // the host as a login instead of joining.
    let challenge = rita
        .ok(REMOTE_CHALLENGE, json!({ "h": h.id(), "o": h.origin }))
        .await["remoteJoinChallenge"]
        .clone();
    let proof = device_login(
        &rita.device_key,
        &rita.state,
        rita.device_id,
        challenge["challengeId"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        &h.id(),
    );
    let body: Value = reqwest::Client::new()
        .post(format!("{}/api", h.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("json");
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{body}"
    );
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) AS n FROM key_challenge WHERE id = $1 AND used_at IS NULL",
            vec![
                challenge["challengeId"]
                    .as_str()
                    .expect("id")
                    .parse::<Uuid>()
                    .expect("uuid")
                    .into()
            ],
        )
        .await,
        1,
        "the refused login leaves the join challenge unused"
    );

    // Nor does a login challenge, or one the host issued for another relay, join.
    let login: Value = reqwest::Client::new()
        .post(format!("{}/api", h.origin))
        .json(&json!({ "query": CHALLENGE }))
        .send()
        .await
        .expect("challenge")
        .json()
        .await
        .expect("json");
    let login = &login["data"]["keyLoginChallenge"];
    let other_relay = h
        .s2s
        .workspace_host
        .join_challenge(
            "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c".to_string(),
            None,
        )
        .await
        .expect("join challenge");
    let join_over = |challenge_id: Uuid, nonce: &str| {
        let proof = device_login(
            &rita.device_key,
            &rita.state,
            rita.device_id,
            challenge_id,
            nonce,
            &h.id(),
        );
        json!({ "input": {
            "hostServerId": h.id(),
            "hostOrigin": h.origin,
            "proof": statement(&proof),
            "delegation": statement(&rita.delegation(&h, &r, &ws, 12)),
        } })
    };
    for (challenge_id, nonce) in [
        (
            login["challengeId"]
                .as_str()
                .expect("id")
                .parse()
                .expect("uuid"),
            login["nonce"].as_str().expect("nonce"),
        ),
        (other_relay.challenge_id, other_relay.nonce.as_str()),
    ] {
        let body = rita
            .post_to(&rita.origin, JOIN, join_over(challenge_id, nonce))
            .await;
        assert_eq!(
            body["errors"][0]["message"], "the challenge is unknown, already used or expired",
            "{body}"
        );
    }
    // The join challenge the relay asked for still joins.
    rita.ok(
        JOIN,
        join_over(
            challenge["challengeId"]
                .as_str()
                .expect("id")
                .parse()
                .expect("uuid"),
            challenge["nonce"].as_str().expect("nonce"),
        ),
    )
    .await;

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_the_relay_cancels_its_subscriptions_at_every_host() {
    let (h, r) = nodes().await;
    let hannah = sign_up(&h, "hannah", 1).await;
    let rita = sign_up(&r, "rita", 2).await;
    let (ws, _, invite) = workspace(&h, &hannah).await;
    rita.join(&h, &r, &ws, Some(&invite)).await;
    assert_eq!(active_delegations(&h, &rita.principal()).await, 1);

    assert_eq!(rita.ok(LEAVE, json!({})).await["leaveRelay"], true);
    assert_eq!(active_delegations(&h, &rita.principal()).await, 0);
    assert_eq!(
        rita.ok(SUBSCRIPTIONS, json!({})).await["remoteSubscriptions"][0]["status"],
        "ended"
    );

    h.stop().await;
    r.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sdl_names_every_remote_workspace_type_and_field() {
    let (first, _) = databases_with(RESET).await;
    let node = start(&first, open_registration).await;
    let sdl = reqwest::get(format!("{}/api/sdl", node.origin))
        .await
        .expect("sdl")
        .text()
        .await
        .expect("text");
    for needle in [
        "type RemoteSubscription",
        "enum RevocationFanoutStatus",
        "PENDING",
        "ACKED",
        "GAVE_UP",
        "type RevocationFanout",
        "type RemoteJoinChallenge",
        "type RemoteJoinResult",
        "input JoinRemoteWorkspaceInput",
        "remoteSubscriptions: [RemoteSubscription!]!",
        "deviceRevocationStatus(deviceId: ID!): [RevocationFanout!]!",
        "remoteJoinChallenge(hostServerId: String!, hostOrigin: String!, inviteToken: String): RemoteJoinChallenge!",
        "joinRemoteWorkspace(input: JoinRemoteWorkspaceInput!): RemoteJoinResult!",
        "renewDelegations(delegations: [SignedStatementInput!]!): [RemoteSubscription!]!",
        "applyRemoteRevocation(revocation: SignedStatementInput!): SignedStatement!",
        "leaveRelay: Boolean!",
    ] {
        assert!(sdl.contains(needle), "the SDL lacks {needle:?}");
    }
    node.stop().await;
}
