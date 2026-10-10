//! Two RIM servers as relays of their users and hosts of their direct spaces: an encrypted DM
//! across relays from creation through Welcome, Commit, a message, an edit and a read state;
//! the request inbox accepted, ignored and blocked; queued host events pushed on `relayStream`
//! and a gap pulled from the host's outbox; forwarded requests with replayed and tampered device
//! requests; and the roles' refusals. Clients talk to their own relay only, over HTTP, and check
//! every host event and forwarded answer against the host's key.
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
use futures_util::StreamExt;
use rim_api::auth_context::AuthActorCtx;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use rim_identity::testing::{SigningKey, TEST_TIME, TestIdentity, device_login, signing_key};
use rim_identity::{
    KeyState, PublicKey, Purpose, SignedStatement, StatementVersion, device_request_payload,
    sign_with_context,
};
use rim_s2s::host::{verify_host_event, verify_host_response};
use rim_server::config::RIMConfig;
use rim_services::relay::service::RelayService;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use support::mls::{MlsDevice, content};
use support::nodes::{Node, databases_with, start};

/// Everything a cross-relay test touches, emptied on both databases first.
const RESET: &str = "TRUNCATE TABLE server_key_log_entry, server_descriptor, peer_key_log_entry, \
     peer_server, s2s_nonce, s2s_outbox, s2s_peer_delivery, s2s_peer_inbound, relay_envelope, \
     remote_identity_cache, key_challenge, \"user\", server RESTART IDENTITY CASCADE";

const SIGNUP: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId } }"#;
const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";
const PUBLISH: &str = r#"
mutation Publish($card: SignedStatementInput!) { publishProfileCard(card: $card) { version } }"#;
const UPLOAD: &str = r#"
mutation Upload($keyPackages: [String!]!) { uploadMlsKeyPackages(keyPackages: $keyPackages) { available } }"#;
const RELAY_INFO: &str = "query { relayInfo { serverId roles directSpaceId } }";
const CREATE: &str = r#"
mutation Create($input: CreateConversationInput!) { createConversation(input: $input) { id } }"#;
const REQUESTS: &str = r#"
query Requests($status: DmRequestStatus) {
  dmRequests(status: $status) {
    id fromPrincipalId hostServerId conversationId status card { payload signature }
    heldItems { id hostServerId workspaceId envelope { payload signature } }
  }
}"#;
const ITEMS: &str = r#"
query Items($after: ID) {
  relayItems(after: $after, limit: 500) {
    items { id hostServerId workspaceId envelope { payload signature } } hasMore
  }
}"#;
const CLAIM: &str = r#"
mutation Claim($input: ClaimMlsKeyPackagesInput!) {
  claimMlsKeyPackages(input: $input) { principalId deviceId keyPackage lastResort }
}"#;
const SUBMIT: &str = r#"
mutation Submit($input: SubmitMlsCommitInput!) { submitMlsCommit(input: $input) { epoch } }"#;
const WELCOMES: &str = "query { myMlsWelcomes { id conversationId epoch message } }";
const ACK_WELCOMES: &str = "mutation Ack($ids: [ID!]!) { ackMlsWelcomes(ids: $ids) }";
const SEND_ENCRYPTED: &str = r#"
mutation Send($input: SendEncryptedMessageInput!) { sendEncryptedMessage(input: $input) { id } }"#;
const EDIT_ENCRYPTED: &str = r#"
mutation Edit($input: EditEncryptedMessageInput!) { editEncryptedMessage(input: $input) { id } }"#;
const MARK_READ: &str = r#"
mutation Read($input: MarkAsReadInput!) { markAsRead(input: $input) }"#;
const HEADS: &str = r#"
query Heads($serverId: ID!, $streams: [String!]!) { streamHeads(serverId: $serverId, streams: $streams) { stream seq } }"#;

fn open_registration(config: &mut RIMConfig) {
    config.open_registration = Some(true);
}

async fn relays() -> (Node, Node) {
    let (a, b) = databases_with(RESET).await;
    (
        start(&a, open_registration).await,
        start(&b, open_registration).await,
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

/// A queued item whose envelope verified under its host's key.
#[derive(Debug, Clone)]
struct Item {
    id: i64,
    stream: String,
    seq: Option<u64>,
    event: Value,
}

/// An answer to a forwarded request, after its host response was checked.
struct Forwarded {
    status: u16,
    body: Value,
    host_signed: bool,
}

/// A person with one device, signed up on their relay over HTTP.
struct Person {
    origin: String,
    http: reqwest::Client,
    session: String,
    csrf: String,
    device_key: SigningKey,
    identity: TestIdentity,
    state: KeyState,
    mls: MlsDevice,
    seq: u64,
}

impl Person {
    fn principal(&self) -> String {
        self.mls.principal_id.clone()
    }

    fn device_id(&self) -> Uuid {
        self.mls.device_id
    }

    async fn post(&self, query: &str, variables: Value) -> Value {
        self.http
            .post(format!("{}/api", self.origin))
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

    /// Run a query on the relay; panics on errors.
    async fn ok(&self, query: &str, variables: Value) -> Value {
        let body = self.post(query, variables).await;
        assert!(body.get("errors").is_none(), "errors: {body}");
        body["data"].clone()
    }

    /// Run a query on the relay that must fail; returns the first error's code.
    async fn err(&self, query: &str, variables: Value) -> String {
        let body = self.post(query, variables).await;
        body["errors"][0]["extensions"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {body}"))
            .to_string()
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Send a device-signed request through the relay to `host`. `sent_body` replaces the
    /// signed body on the wire; `seq` reuses a sequence number.
    async fn forward(
        &mut self,
        host: &Node,
        method: &str,
        path: &str,
        body: Vec<u8>,
        sent_body: Option<Vec<u8>>,
        seq: Option<u64>,
    ) -> (Forwarded, u64) {
        let seq = seq.unwrap_or_else(|| self.next_seq());
        let payload = device_request_payload(
            &self.principal(),
            self.device_id(),
            &host.id(),
            method,
            path,
            seq,
            &body,
            TEST_TIME,
        );
        let signature = sign_with_context(
            Purpose::DeviceRequest.context(),
            &self.device_key,
            payload.clone(),
        )
        .signature;
        let url = format!("{}/api/remote/{}{path}", self.origin, host.id());
        let request = match method {
            "POST" => self
                .http
                .post(url)
                .header("content-type", "application/json")
                .body(sent_body.unwrap_or(body)),
            _ => self.http.get(url),
        };
        let response = request
            .header(
                "cookie",
                format!("rim_session={}; rim_csrf={}", self.session, self.csrf),
            )
            .header("x-csrf-token", &self.csrf)
            .header("rim-device-request", URL_SAFE_NO_PAD.encode(&payload))
            .header("rim-device-signature", signature)
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
        let host_response = header("rim-host-response").zip(header("rim-host-signature"));
        let bytes = response.bytes().await.expect("body");
        let host_signed = match host_response {
            Some((encoded, signature)) => {
                let statement = SignedStatement {
                    payload: String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).expect("b64"))
                        .expect("utf8"),
                    signature,
                };
                let verified =
                    verify_host_response(&host_key(host), &statement).expect("host-signed");
                assert_eq!(verified.host, host.id());
                assert!(
                    verified.check(&payload, status, &bytes),
                    "the host response binds this request, status and body"
                );
                true
            }
            None => false,
        };
        (
            Forwarded {
                status,
                body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                host_signed,
            },
            seq,
        )
    }

    /// Run GraphQL at `host` through the relay; panics unless it succeeded.
    async fn remote(&mut self, host: &Node, query: &str, variables: Value) -> Value {
        let body =
            serde_json::to_vec(&json!({ "query": query, "variables": variables })).expect("json");
        let (answer, _) = self.forward(host, "POST", "/api", body, None, None).await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert!(answer.host_signed);
        assert!(answer.body.get("errors").is_none(), "{}", answer.body);
        answer.body["data"].clone()
    }

    /// The relay's queued items, each envelope verified under `host`'s key.
    async fn items(&self, host: &Node, after: Option<i64>) -> Vec<Item> {
        let data = self
            .ok(ITEMS, json!({ "after": after.map(|a| a.to_string()) }))
            .await;
        data["relayItems"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| verify_item(host, item))
            .collect()
    }

    /// Wait for a queued item of `kind` that `matches` it.
    async fn item_of(&self, host: &Node, kind: &str, matches: impl Fn(&Item) -> bool) -> Item {
        eventually(kind, || async {
            self.items(host, None)
                .await
                .into_iter()
                .find(|i| i.event["type"] == kind && matches(i))
        })
        .await
    }
}

/// Check an item's envelope against the host key and read it.
fn verify_item(host: &Node, item: &Value) -> Item {
    assert_eq!(item["hostServerId"], json!(host.id()));
    let envelope = signed(&item["envelope"]);
    let event = verify_host_event(&host_key(host), &envelope).expect("a host-signed event");
    assert_eq!(event.host, host.id());
    assert_eq!(json!(event.workspace_id), item["workspaceId"]);
    if let Some(conversation) = event.event.get("conversation_id").and_then(Value::as_str)
        && event.stream.starts_with("c:")
    {
        assert_eq!(event.stream, format!("c:{conversation}"));
    }
    Item {
        id: item["id"].as_str().expect("id").parse().expect("decimal"),
        stream: event.stream,
        seq: event.seq,
        event: event.event,
    }
}

/// Sign up `name` on `node` with a key, publish a card naming the node, and upload key packages.
async fn sign_up(node: &Node, name: &str, seed: u8) -> Person {
    sign_up_with_card(node, name, seed, 1).await
}

/// [`sign_up`] publishing card `card_version`. A seed is one person with one device everywhere.
async fn sign_up_with_card(node: &Node, name: &str, seed: u8, card_version: u64) -> Person {
    let http = reqwest::Client::new();
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let inception = identity.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::from_u128(0x1000 + u128::from(seed));
    let device_key = signing_key(seed.wrapping_add(50));
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
    let mut cookies = std::collections::HashMap::new();
    for value in response.headers().get_all("set-cookie") {
        let text = value.to_str().expect("cookie");
        if let Some((name, rest)) = text.split_once('=') {
            cookies.insert(
                name.to_string(),
                rest.split(';').next().unwrap_or_default().to_string(),
            );
        }
    }
    let body: Value = response.json().await.expect("json");
    assert!(body.get("errors").is_none(), "signup: {body}");
    let person = Person {
        origin: node.origin.clone(),
        http,
        session: cookies.remove("rim_session").expect("session cookie"),
        csrf: cookies.remove("rim_csrf").expect("csrf cookie"),
        mls: MlsDevice::new(
            device_id,
            state.principal_id.clone(),
            device_key.clone(),
            signing_key(seed.wrapping_add(150)),
        ),
        device_key,
        identity,
        state,
        seq: 1_791_504_000_000_000 + u64::from(seed) * 1_000_000_000,
    };
    person
        .publish_card(card_version, &[node.origin.as_str()])
        .await;
    let packages = vec![
        person.mls.key_package(false),
        person.mls.key_package(false),
        person.mls.key_package(true),
    ];
    person.ok(UPLOAD, json!({ "keyPackages": packages })).await;
    person
}

impl Person {
    async fn publish_card(&self, version: u64, relays: &[&str]) {
        let card = self.identity.profile_card(
            &self.state,
            version,
            &format!("Person {}", &self.principal()[6..10]),
            relays,
            &[(self.device_id(), &self.device_key)],
        );
        self.ok(PUBLISH, json!({ "card": statement(&card) })).await;
    }

    async fn direct_space(&self) -> String {
        self.ok(RELAY_INFO, json!({})).await["relayInfo"]["directSpaceId"]
            .as_str()
            .expect("direct space")
            .to_string()
    }

    /// Start a conversation in the direct space with someone on another relay.
    async fn start_dm(&self, other: &Person, other_relay: &Node, encrypted: bool) -> String {
        let direct = self.direct_space().await;
        self.ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "encrypted": encrypted,
                "remoteMembers": [{ "principalId": other.principal(), "relayUrls": [other_relay.origin] }],
            } }),
        )
        .await["createConversation"]["id"]
            .as_str()
            .expect("conversation id")
            .to_string()
    }

    /// Create the encrypted conversation's group with `other`'s device, claiming their key
    /// package through their relay.
    async fn create_group_with(&mut self, other: &Person, direct: &str, conv: &str) {
        let claimed = self
            .ok(
                CLAIM,
                json!({ "input": {
                    "serverId": direct,
                    "conversationId": conv,
                    "devices": [{ "principalId": other.principal(), "deviceId": other.device_id() }],
                } }),
            )
            .await["claimMlsKeyPackages"]
            .clone();
        let other_principal = other.principal();
        let (commit, welcome) = self.mls.create_group(
            conv.parse().expect("uuid"),
            &[(
                other_principal.as_str(),
                other.device_id(),
                claimed[0]["keyPackage"].as_str().expect("package"),
            )],
        );
        self.ok(
            SUBMIT,
            json!({ "input": {
                "serverId": direct, "conversationId": conv, "incarnation": 1, "epoch": 0,
                "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
            } }),
        )
        .await;
        self.mls.merge_pending();
    }

    /// Send an encrypted message; returns its id.
    async fn send_encrypted(&mut self, direct: &str, conv: &str, body: &str) -> Uuid {
        let id = Uuid::now_v7();
        let envelope = self
            .mls
            .encrypt(id, &content(id, 0, body, &[]), [6; 32], &[]);
        self.ok(
            SEND_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": id, "envelope": envelope } }),
        )
        .await;
        id
    }

    async fn pending_request(&self, from: &Person) -> Value {
        eventually("a DM request", || async {
            let data = self.ok(REQUESTS, json!({ "status": "PENDING" })).await;
            data["dmRequests"]
                .as_array()
                .and_then(|r| {
                    r.iter()
                        .find(|r| r["fromPrincipalId"] == json!(from.principal()))
                })
                .cloned()
        })
        .await
    }
}

/// Wait until `host` delivered everything it queued, after giving its writer a moment.
async fn drained(host: &Node) {
    tokio::time::sleep(Duration::from_millis(500)).await;
    eventually("the outbox to drain", || async {
        let row = host
            .db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT (SELECT COALESCE(max(id), 0) FROM s2s_outbox) <= \
                 (SELECT COALESCE(max(delivered_through), 0) FROM s2s_peer_delivery) AS done",
            ))
            .await
            .expect("query")
            .expect("row");
        row.try_get::<bool>("", "done").expect("done").then_some(())
    })
    .await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_encrypted_dm_crosses_relays_from_invite_to_read_state() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;

    // Alice's relay fetches Bob's identity from his relay, verified.
    let identity = alice
        .ok(
            "query($p: String!, $r: String) { remoteIdentity(principalId: $p, relayUrl: $r) { principalId servedBy keyLog { payload } devices { deviceId mlsActive } card { payload } } }",
            json!({ "p": bob.principal(), "r": b.origin }),
        )
        .await["remoteIdentity"]
        .clone();
    assert_eq!(identity["servedBy"], json!(b.id()));
    assert_eq!(
        identity["devices"][0]["deviceId"],
        json!(bob.device_id().to_string())
    );
    assert_eq!(identity["devices"][0]["mlsActive"], json!(true));
    assert!(identity["card"]["payload"].is_string());

    // Alice starts an encrypted DM in her relay's direct space.
    let direct = alice.direct_space().await;
    let conv = alice.start_dm(&bob, &b, true).await;

    // Bob's relay files it as a request: Alice is not his contact.
    let request = bob.pending_request(&alice).await;
    assert_eq!(request["hostServerId"], json!(a.id()));
    assert_eq!(request["conversationId"], json!(conv));
    let held: Vec<Item> = request["heldItems"]
        .as_array()
        .expect("held")
        .iter()
        .map(|i| verify_item(&a, i))
        .collect();
    assert!(!held.is_empty() && held.len() <= 3);
    let invite = &held[0];
    assert_eq!(invite.event["type"], "DmInvite");
    assert_eq!(invite.stream, format!("u:{}", bob.principal()));
    assert_eq!(invite.event["from_principal_id"], json!(alice.principal()));
    assert_eq!(invite.event["from_card"], request["card"]);
    assert!(
        bob.items(&a, None).await.is_empty(),
        "held items wait for the request"
    );

    let accepted = bob
        .ok(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(accepted["acceptDmRequest"]["status"], "ACCEPTED");
    let contacts = bob
        .ok("query { contacts { principalId relayUrls } }", json!({}))
        .await;
    assert_eq!(
        contacts["contacts"][0]["principalId"],
        json!(alice.principal())
    );
    assert_eq!(contacts["contacts"][0]["relayUrls"], json!([a.origin]));
    let released = bob.items(&a, None).await;
    assert_eq!(released[0].event["type"], "DmInvite");
    let created = released
        .iter()
        .find(|i| i.event["type"] == "ConversationCreated")
        .expect("ConversationCreated");
    assert_eq!(created.stream, format!("c:{conv}"));
    assert_eq!(created.seq, Some(1));

    // Alice claims Bob's device through his relay: he accepted her, so a one-time package.
    let claimed = alice
        .ok(
            CLAIM,
            json!({ "input": {
                "serverId": direct,
                "conversationId": conv,
                "devices": [{ "principalId": bob.principal(), "deviceId": bob.device_id() }],
            } }),
        )
        .await["claimMlsKeyPackages"]
        .clone();
    assert_eq!(claimed[0]["lastResort"], json!(false));
    let mut alice = alice;
    let conversation_id: Uuid = conv.parse().expect("uuid");
    let bob_principal = bob.principal();
    let (commit, welcome) = alice.mls.create_group(
        conversation_id,
        &[(
            bob_principal.as_str(),
            bob.device_id(),
            claimed[0]["keyPackage"].as_str().expect("package"),
        )],
    );
    alice
        .ok(
            SUBMIT,
            json!({ "input": {
                "serverId": direct, "conversationId": conv, "incarnation": 1, "epoch": 0,
                "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
            } }),
        )
        .await;
    alice.mls.merge_pending();

    // The Welcome notice reaches Bob's device through his relay; he fetches the Welcome from
    // the home, through his relay, and acknowledges it.
    let notice = bob
        .item_of(&a, "MlsWelcomeAvailable", |i| {
            i.event["device_id"] == json!(bob.device_id().to_string())
        })
        .await;
    assert_eq!(notice.stream, format!("u:{}", bob.principal()));
    let welcomes = bob.remote(&a, WELCOMES, json!({})).await["myMlsWelcomes"].clone();
    assert_eq!(welcomes[0]["conversationId"], json!(conv));
    bob.mls
        .join(welcomes[0]["message"].as_str().expect("welcome"));
    let acked = bob
        .remote(&a, ACK_WELCOMES, json!({ "ids": [welcomes[0]["id"]] }))
        .await;
    assert_eq!(acked["ackMlsWelcomes"], json!(1));

    // Alice writes and edits; Bob decrypts both from host-signed events on his relay.
    let message_id = Uuid::now_v7();
    let body = content(message_id, 0, "hello across relays", &[]);
    let envelope = alice.mls.encrypt(message_id, &body, [3; 32], &[]);
    alice
        .ok(
            SEND_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    let new_message = bob
        .item_of(&a, "NewMessage", |i| {
            i.event["message"]["id"] == json!(message_id.to_string())
        })
        .await;
    let ciphertext = new_message.event["message"]["encryption"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    let (received, _) = bob.mls.decrypt(&ciphertext, &message_id.to_string());
    assert_eq!(received, body);

    let edited = content(message_id, 1, "hello across relays, edited", &[]);
    let envelope = alice.mls.encrypt(message_id, &edited, [4; 32], &[]);
    alice
        .ok(
            EDIT_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    let updated = bob
        .item_of(&a, "MessageUpdated", |i| {
            i.event["message"]["id"] == json!(message_id.to_string())
        })
        .await;
    assert!(updated.seq > new_message.seq, "one stream, in order");
    let ciphertext = updated.event["message"]["encryption"]["ciphertext"]
        .as_str()
        .expect("ciphertext")
        .to_string();
    let (received, _) = bob.mls.decrypt(&ciphertext, &message_id.to_string());
    assert_eq!(received, edited);

    // Bob marks it read at the home; the read state comes back on his own stream.
    let read = bob
        .remote(
            &a,
            MARK_READ,
            json!({ "input": { "workspaceId": direct, "conversationId": conv, "readAt": chrono::Utc::now().to_rfc3339() } }),
        )
        .await;
    assert_eq!(read["markAsRead"], json!(true));
    let state = bob
        .item_of(&a, "ReadStateUpdated", |i| {
            i.event["conversation_id"] == json!(conv)
        })
        .await;
    assert_eq!(state.stream, format!("u:{}", bob.principal()));

    // Stream heads at the home: his streams, not the members-only `w`.
    let heads = bob
        .remote(
            &a,
            HEADS,
            json!({ "serverId": direct, "streams": [format!("c:{conv}"), format!("u:{}", bob.principal()), "w"] }),
        )
        .await["streamHeads"]
        .clone();
    let heads = heads.as_array().expect("heads");
    assert_eq!(heads.len(), 2, "a guest does not see w");
    assert_eq!(
        heads[0]["seq"],
        json!(updated.seq.expect("seq").to_string())
    );

    // Acknowledged items leave the queue once every device passed them.
    let last = bob.items(&a, None).await.last().expect("items").id;
    bob.ok(
        "mutation($u: ID!) { ackRelayItems(upTo: $u) }",
        json!({ "u": last.to_string() }),
    )
    .await;
    assert!(bob.items(&a, None).await.is_empty());
    b.s2s.relay.cleanup().await.expect("cleanup");
    let left =
        b.db.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM relay_item WHERE request_id IS NULL",
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(left, 0);

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dm_requests_are_held_for_strangers_ignored_and_blocked() {
    let (a, b) = relays().await;
    let mut alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;

    // A stranger's claim gets the last-resort package only.
    let conv = alice.start_dm(&bob, &b, true).await;
    let direct = alice.direct_space().await;
    let request = bob.pending_request(&alice).await;
    let claimed = alice
        .ok(
            CLAIM,
            json!({ "input": {
                "serverId": direct,
                "conversationId": conv,
                "devices": [{ "principalId": bob.principal(), "deviceId": bob.device_id() }],
            } }),
        )
        .await["claimMlsKeyPackages"]
        .clone();
    assert_eq!(claimed[0]["lastResort"], json!(true));
    let bob_principal = bob.principal();
    let (commit, welcome) = alice.mls.create_group(
        conv.parse().expect("uuid"),
        &[(
            bob_principal.as_str(),
            bob.device_id(),
            claimed[0]["keyPackage"].as_str().expect("package"),
        )],
    );
    alice
        .ok(
            SUBMIT,
            json!({ "input": {
                "serverId": direct, "conversationId": conv, "incarnation": 1, "epoch": 0,
                "cipherSuite": 0x004f, "commit": commit, "welcome": welcome,
            } }),
        )
        .await;
    eventually("three held items", || async {
        let held = bob.pending_request(&alice).await["heldItems"]
            .as_array()
            .expect("held")
            .len();
        assert!(held <= 3, "later items are dropped");
        (held == 3).then_some(())
    })
    .await;
    drained(&a).await;

    // Ignoring deletes the held items and drops what follows.
    let ignored = bob
        .ok(
            "mutation($id: ID!) { ignoreDmRequest(id: $id) { status heldItems { id hostServerId workspaceId envelope { payload signature } } } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(ignored["ignoreDmRequest"]["status"], "IGNORED");
    let kept = ignored["ignoreDmRequest"]["heldItems"]
        .as_array()
        .expect("held");
    assert_eq!(kept.len(), 1, "only the invite stays");
    assert_eq!(verify_item(&a, &kept[0]).event["type"], "DmInvite");
    let code = bob
        .err(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert_eq!(code, "CONFLICT");
    alice.mls.merge_pending();
    let message_id = Uuid::now_v7();
    let envelope = alice.mls.encrypt(
        message_id,
        &content(message_id, 0, "hi?", &[]),
        [5; 32],
        &[],
    );
    alice
        .ok(
            SEND_ENCRYPTED,
            json!({ "input": { "serverId": direct, "conversationId": conv, "messageId": message_id, "envelope": envelope } }),
        )
        .await;
    drained(&a).await;
    assert!(
        bob.items(&a, None).await.is_empty(),
        "an ignored conversation stays out"
    );

    // Blocking ignores pending requests and refuses later invites.
    carol.start_dm(&bob, &b, true).await;
    let from_carol = bob.pending_request(&carol).await;
    let blocked = bob
        .ok(
            "mutation($p: String!) { blockPrincipal(principalId: $p) }",
            json!({ "p": carol.principal() }),
        )
        .await;
    assert_eq!(blocked["blockPrincipal"], json!(true));
    let requests = bob.ok(REQUESTS, json!({ "status": "IGNORED" })).await;
    assert!(
        requests["dmRequests"]
            .as_array()
            .expect("requests")
            .iter()
            .any(|r| r["id"] == from_carol["id"])
    );
    let direct = carol.direct_space().await;
    carol
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [],
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
                "title": "group",
                "encrypted": true,
            } }),
        )
        .await;
    drained(&a).await;
    let pending = bob.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    assert_eq!(
        pending["dmRequests"],
        json!([]),
        "a blocked sender files nothing"
    );
    let report = bob
        .ok(
            "mutation($id: ID!, $r: String!) { reportDmRequest(id: $id, reason: $r) }",
            json!({ "id": from_carol["id"], "r": "spam" }),
        )
        .await;
    assert_eq!(report["reportDmRequest"], json!(true));
    let unblocked = bob
        .ok(
            "mutation($p: String!) { unblockPrincipal(principalId: $p) }",
            json!({ "p": carol.principal() }),
        )
        .await;
    assert_eq!(unblocked["unblockPrincipal"], json!(true));

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_stream_pushes_host_events_and_a_gap_is_pulled_from_the_outbox() {
    let (a, b) = relays().await;
    let mut alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    // A contact's conversation flows without a request.
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": alice.principal(), "r": [a.origin] }),
    )
    .await;

    let bob_user = user_id(&b, &bob.principal()).await;
    let bob_device = device_key_id(&b, bob.device_id()).await;
    let mut stream =
        b.schema
            .execute_stream(Request::new("subscription { relayStream }").data(
                AuthActorCtx::new(Some(bob_user), None).with_device_key_id(Some(bob_device)),
            ));

    let conv = alice.start_dm(&bob, &b, true).await;
    let direct = alice.direct_space().await;
    alice.create_group_with(&bob, &direct, &conv).await;
    let first_id = alice.send_encrypted(&direct, &conv, "first").await;
    let mut seen = Vec::new();
    while !seen.iter().any(|i: &Item| i.event["type"] == "NewMessage") {
        let response = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("an item in time")
            .expect("the stream stays open");
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let text = response.data.into_json().expect("json")["relayStream"]
            .as_str()
            .expect("json string")
            .to_string();
        let pushed: Value = serde_json::from_str(&text).expect("item json");
        assert_eq!(pushed["type"], "RelayItem");
        seen.push(verify_item(&a, &pushed));
    }
    assert!(
        seen.iter().any(|i| i.event["type"] == "DmInvite"),
        "a contact's invite flows"
    );
    let first = seen
        .iter()
        .find(|i| i.event["type"] == "NewMessage")
        .expect("NewMessage")
        .clone();
    assert_eq!(first.event["message"]["id"], json!(first_id.to_string()));

    // Bob's relay misses a delivery; it pulls the gap from Alice's relay's outbox. Holding
    // deliveries waits for the sender to finish, so no acknowledgement in flight resets it.
    drained(&a).await;
    a.db.execute_unprepared(
        "UPDATE s2s_peer_delivery SET next_attempt_at = now() + interval '1 hour', failures = 9",
    )
    .await
    .expect("hold deliveries");
    let second_id = json!(
        alice
            .send_encrypted(&direct, &conv, "second")
            .await
            .to_string()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = bob.items(&a, Some(first.id)).await;
    assert!(
        before.iter().all(|i| i.event["message"]["id"] != second_id),
        "the delivery is held"
    );
    let pulled = b.s2s.relay.pull_outbox(a.id()).await.expect("pull");
    assert!(pulled >= 1);
    let after = bob.items(&a, Some(first.id)).await;
    let second = after
        .iter()
        .find(|i| i.event["message"]["id"] == second_id)
        .expect("the pulled item");
    assert_eq!(
        second.seq,
        first.seq.map(|s| s + 1),
        "the conversation stream has no gap"
    );
    assert_eq!(
        b.s2s.relay.pull_outbox(a.id()).await.expect("pull"),
        0,
        "nothing above what was received"
    );

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_requests_refuse_replays_tampering_and_relays_a_card_does_not_name() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    alice.start_dm(&bob, &b, true).await;
    bob.pending_request(&alice).await;
    let direct = alice.direct_space().await;

    let query = json!({ "query": "query($s: ID!) { streamHeads(serverId: $s, streams: [\"w\"]) { seq } }", "variables": { "s": direct } });
    let body = serde_json::to_vec(&query).expect("json");
    let (answer, seq) = bob
        .forward(&a, "POST", "/api", body.clone(), None, None)
        .await;
    assert_eq!(answer.status, 200);
    assert!(answer.host_signed);

    // The same seq again: a replay, with the highest seq so the client moves past it.
    let (replayed, _) = bob
        .forward(&a, "POST", "/api", body.clone(), None, Some(seq))
        .await;
    assert_eq!(replayed.status, 409);
    assert!(replayed.host_signed, "refusals are host-signed too");
    assert_eq!(replayed.body["error"], "replay");
    assert_eq!(replayed.body["highestSeq"], json!(seq.to_string()));

    // Another body than the one signed.
    let (tampered, _) = bob
        .forward(
            &a,
            "POST",
            "/api",
            body.clone(),
            Some(br#"{"query":"query { relayInfo { serverId } }"}"#.to_vec()),
            None,
        )
        .await;
    assert_eq!(tampered.status, 403);
    assert_eq!(tampered.body["error"], "forbidden");

    // Account mutations stay at a person's own relay.
    let card = bob.identity.profile_card(
        &bob.state,
        9,
        "Bob",
        &[b.origin.as_str()],
        &[(bob.device_id(), &bob.device_key)],
    );
    let publish =
        serde_json::to_vec(&json!({ "query": PUBLISH, "variables": { "card": statement(&card) } }))
            .expect("json");
    let (refused, _) = bob.forward(&a, "POST", "/api", publish, None, None).await;
    assert_eq!(refused.status, 200);
    assert_eq!(refused.body["errors"][0]["extensions"]["code"], "FORBIDDEN");

    // A request without the device headers never reaches the host's handlers.
    let raw = reqwest::Client::new()
        .post(format!("{}/api/remote/{}/api", b.origin, a.id()))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", bob.session, bob.csrf),
        )
        .header("x-csrf-token", &bob.csrf)
        .body(body.clone())
        .send()
        .await
        .expect("send");
    assert_eq!(raw.status(), 400);

    // Once Bob's card stops naming his relay, the host refuses what it forwards.
    bob.publish_card(2, &["https://elsewhere.example"]).await;
    let bob_on_a = user_id(&a, &bob.principal()).await;
    eventually("the pushed card", || async {
        let row =
            a.db.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT version FROM profile_card WHERE user_id = $1",
                [Uuid::from(bob_on_a).into()],
            ))
            .await
            .expect("query")?;
        (row.try_get::<i64>("", "version").expect("version") == 2).then_some(())
    })
    .await;
    let (stranger, _) = bob.forward(&a, "POST", "/api", body, None, None).await;
    assert_eq!(stranger.status, 403);
    assert_eq!(stranger.body["error"], "not_delegated");

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_and_host_roles_refuse_what_they_do_not_run() {
    let (first, second) = databases_with(RESET).await;
    let host = start(&first, |c| {
        open_registration(c);
        c.role = Some("host".to_string());
    })
    .await;
    let relay = start(&second, |c| {
        open_registration(c);
        c.role = Some("relay".to_string());
    })
    .await;
    let carol = sign_up(&relay, "carol", 3).await;
    let dave = sign_up(&host, "dave", 4).await;

    // A relay hosts no new workspaces; a host keeps no contacts.
    let code = carol
        .err(
            "mutation { createServer(input: { name: \"w\" }) { id } }",
            json!({}),
        )
        .await;
    assert_eq!(code, "FORBIDDEN");
    let code = dave
        .err("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(code, "FORBIDDEN");
    let info = dave.ok(RELAY_INFO, json!({})).await;
    assert_eq!(info["relayInfo"]["roles"], json!(["host"]));

    // A host forwards nothing for its users and takes no deliveries.
    let remote = reqwest::Client::new()
        .post(format!("{}/api/remote/{}/api", host.origin, relay.id()))
        .header(
            "cookie",
            format!("rim_session={}; rim_csrf={}", dave.session, dave.csrf),
        )
        .header("x-csrf-token", &dave.csrf)
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(remote.status(), 403);
    relay
        .s2s
        .state
        .peers
        .pin(&host.origin, Some(&host.id()))
        .await
        .expect("pin");
    let answer = relay
        .s2s
        .client
        .send(rim_services::s2s::dto::S2sRequest {
            peer_server_id: host.id(),
            method: rim_services::s2s::dto::S2sMethod::Post,
            path: "/deliver".into(),
            body: br#"{"items":[]}"#.to_vec(),
            content_type: None,
            headers: vec![],
        })
        .await
        .expect("a signed answer");
    assert_eq!(answer.status, 503);
    assert_eq!(answer.error_code().as_deref(), Some("role_disabled"));
    // A relay serves its users' identities to any pinned peer.
    host.s2s
        .state
        .peers
        .pin(&relay.origin, Some(&relay.id()))
        .await
        .expect("pin");
    let identity = host
        .s2s
        .client
        .send(rim_services::s2s::dto::S2sRequest {
            peer_server_id: relay.id(),
            method: rim_services::s2s::dto::S2sMethod::Get,
            path: format!("/principals/{}", carol.principal()),
            body: vec![],
            content_type: None,
            headers: vec![],
        })
        .await
        .expect("a signed answer");
    assert_eq!(identity.status, 200);
    let document: Value = serde_json::from_slice(&identity.body).expect("json");
    assert_eq!(document["principalId"], json!(carol.principal()));

    host.stop().await;
    relay.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_user_signing_up_here_becomes_local_and_leaving_a_relay_clears_it() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;
    alice.start_dm(&bob, &b, true).await;
    bob.pending_request(&alice).await;
    let remote = user_id(&a, &bob.principal()).await;
    let row = |node: &Node, id: UserId| {
        let db = node.db.clone();
        async move {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"SELECT kind, username, (SELECT count(*) FROM server_member m
                         JOIN server s ON s.id = m.server_id AND s.kind = 'direct'
                         WHERE m.user_id = u.id) AS direct
                       FROM "user" u WHERE id = $1"#,
                    [Uuid::from(id).into()],
                ))
                .await
                .expect("query")
                .expect("row");
            (
                row.try_get::<String>("", "kind").expect("kind"),
                row.try_get::<String>("", "username").expect("username"),
                row.try_get::<i64>("", "direct").expect("direct"),
            )
        }
    };
    let (kind, username, direct) = row(&a, remote).await;
    assert_eq!(kind, "remote");
    assert!(username.starts_with("r_"));
    assert_eq!(direct, 1);

    // Bob signs up on Alice's relay with the same key and device: the row becomes his.
    sign_up_with_card(&a, "bobby", 2, 2).await;
    assert_eq!(user_id(&a, &bob.principal()).await, remote);
    assert_eq!(
        row(&a, remote).await,
        ("local".to_string(), "bobby".to_string(), 1)
    );

    // A successor names the conversation it replaces.
    let dave = sign_up(&b, "dave", 5).await;
    let direct = alice.direct_space().await;
    let replaced = Uuid::now_v7();
    let successor = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "remoteMembers": [{ "principalId": dave.principal(), "relayUrls": [b.origin] }],
                "encrypted": true,
                "predecessor": { "hostServerId": b.id(), "conversationId": replaced },
            } }),
        )
        .await["createConversation"]["id"]
        .clone();
    let predecessor = alice
        .ok(
            "query($s: ID!, $c: ID!) { conversation(serverId: $s, conversationId: $c) { conversationType predecessor { hostServerId conversationId } } }",
            json!({ "s": direct, "c": successor }),
        )
        .await["conversation"]
        .clone();
    assert_eq!(
        predecessor["conversationType"], "DIRECT",
        "one remote member is a DM"
    );
    assert_eq!(
        predecessor["predecessor"],
        json!({ "hostServerId": b.id(), "conversationId": replaced.to_string() })
    );

    // Leaving his own relay deletes his requests and contacts there.
    bob.ok(
        "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
        json!({ "p": alice.principal(), "r": [a.origin] }),
    )
    .await;
    let left = bob.ok("mutation { leaveRelay }", json!({})).await;
    assert_eq!(left["leaveRelay"], json!(true));
    assert_eq!(
        bob.ok("query { contacts { principalId } }", json!({}))
            .await["contacts"],
        json!([])
    );
    assert_eq!(bob.ok(REQUESTS, json!({})).await["dmRequests"], json!([]));

    a.stop().await;
    b.stop().await;
}

/// The conversations `person` sees in `direct`'s list.
async fn listed(person: &Person, direct: &str) -> Vec<String> {
    person
        .ok(
            "query($s: ID!) { inboxConversations(serverId: $s, dir: AFTER, limit: 50) { items { id } } }",
            json!({ "s": direct }),
        )
        .await["inboxConversations"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|c| c["id"].as_str().expect("id").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strangers_dm_with_a_member_of_the_home_lands_in_its_request_inbox() {
    let (a, b) = relays().await;
    let alice = sign_up(&a, "alice", 1).await;
    let mut bob = sign_up(&b, "bob", 2).await;
    let carol = sign_up(&a, "carol", 3).await;
    let mut dave = sign_up(&b, "dave", 5).await;
    // Carol's DMs make Bob and Dave members of A's direct space.
    carol.start_dm(&bob, &b, true).await;
    carol.start_dm(&dave, &b, true).await;
    alice
        .ok(
            "mutation($p: String!, $r: [String!]!) { addContact(principalId: $p, relayUrls: $r) { principalId } }",
            json!({ "p": dave.principal(), "r": [b.origin] }),
        )
        .await;
    let direct = alice.direct_space().await;

    // A conversation with someone on another relay is encrypted only.
    let code = alice
        .err(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
            } }),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    let message = alice
        .post(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "encrypted": false,
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
            } }),
        )
        .await["errors"][0]["message"]
        .clone();
    assert_eq!(
        message,
        "conversations with people on other servers are encrypted"
    );

    let create_with_alice = json!({ "input": {
        "serverId": direct,
        "encrypted": true,
        "remoteMembers": [{ "principalId": alice.principal(), "relayUrls": [a.origin] }],
    } });

    // Bob, a stranger to Alice, starts a DM with her at her own relay: it is a request.
    let from_bob =
        bob.remote(&a, CREATE, create_with_alice.clone()).await["createConversation"]["id"]
            .as_str()
            .expect("id")
            .to_string();
    let request = alice.pending_request(&bob).await;
    assert_eq!(request["hostServerId"], json!(a.id()));
    assert_eq!(request["conversationId"], json!(from_bob));
    let invite = verify_item(&a, &request["heldItems"][0]);
    assert_eq!(invite.event["type"], "DmInvite");
    assert_eq!(invite.stream, format!("u:{}", alice.principal()));
    assert!(
        !listed(&alice, &direct).await.contains(&from_bob),
        "a request is not a live conversation"
    );

    // Dave, her contact, starts one too: it is live at once.
    let from_dave = dave.remote(&a, CREATE, create_with_alice).await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pending = alice.ok(REQUESTS, json!({ "status": "PENDING" })).await;
    assert!(
        pending["dmRequests"]
            .as_array()
            .expect("requests")
            .iter()
            .all(|r| r["fromPrincipalId"] != json!(dave.principal())),
        "a contact files no request"
    );
    assert!(listed(&alice, &direct).await.contains(&from_dave));

    // Accepting Bob's request makes his conversation live.
    alice
        .ok(
            "mutation($id: ID!) { acceptDmRequest(id: $id) { status } }",
            json!({ "id": request["id"] }),
        )
        .await;
    assert!(listed(&alice, &direct).await.contains(&from_bob));

    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sdl_names_every_relay_type_and_field() {
    let (first, _) = databases_with(RESET).await;
    let node = start(&first, open_registration).await;
    let sdl = reqwest::get(format!("{}/api/sdl", node.origin))
        .await
        .expect("sdl")
        .text()
        .await
        .expect("text");
    for needle in [
        "type RelayInfo",
        "type RemoteServer",
        "type RemotePrincipalDevice",
        "type RemoteIdentity",
        "type Contact",
        "enum DmRequestStatus",
        "type RelayItem ",
        "type RelayItemPage",
        "type DmRequest ",
        "type StreamHead",
        "input RemoteMemberInput",
        "input PredecessorInput",
        "type ConversationPredecessor",
        "predecessor: ConversationPredecessor",
        "remoteMembers: [RemoteMemberInput!]",
        "predecessor: PredecessorInput",
        "relayInfo: RelayInfo!",
        "remoteServer(origin: String!): RemoteServer!",
        "remoteIdentity(principalId: String!, relayUrl: String): RemoteIdentity!",
        "contacts: [Contact!]!",
        "dmRequests(status: DmRequestStatus): [DmRequest!]!",
        "relayItems(after: ID, limit: Int): RelayItemPage!",
        "streamHeads(serverId: ID!, streams: [String!]!): [StreamHead!]!",
        "addContact(principalId: String!, relayUrls: [String!]!): Contact!",
        "removeContact(principalId: String!): Boolean!",
        "acceptDmRequest(id: ID!): DmRequest!",
        "ignoreDmRequest(id: ID!): DmRequest!",
        "blockPrincipal(principalId: String!): Boolean!",
        "unblockPrincipal(principalId: String!): Boolean!",
        "reportDmRequest(id: ID!, reason: String!): Boolean!",
        "ackRelayItems(upTo: ID!): Boolean!",
        "leaveRelay: Boolean!",
        "relayStream(after: ID): String!",
    ] {
        assert!(sdl.contains(needle), "the SDL lacks {needle:?}");
    }
    node.stop().await;
}
