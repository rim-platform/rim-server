//! People on two relays for the server-to-server tests: key-only signups over HTTP, cards and
//! key packages, device-signed requests through the relay, queued host events checked against
//! the host's key, and the queries the tests share.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use rim_identity::testing::{SigningKey, TEST_TIME, TestIdentity, device_login, signing_key};
use rim_identity::{
    KeyState, PublicKey, Purpose, SignedStatement, StatementVersion, device_request_payload,
    sign_with_context,
};
use rim_s2s::host::{verify_host_event, verify_host_response};
use rim_server::config::RIMConfig;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::mls::{MlsDevice, content};
use super::nodes::{Node, databases_with, start};

/// Everything a cross-relay test touches, emptied on both databases first.
pub const RESET: &str = "TRUNCATE TABLE server_key_log_entry, server_descriptor, peer_key_log_entry, \
     peer_server, s2s_nonce, s2s_outbox, s2s_peer_delivery, s2s_peer_inbound, relay_envelope, \
     remote_identity_cache, key_challenge, relay_principal_block, relay_abuse_report, \"user\", \
     server RESTART IDENTITY CASCADE";

pub const SIGNUP: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId } }"#;
pub const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";
pub const PUBLISH: &str = r#"
mutation Publish($card: SignedStatementInput!) { publishProfileCard(card: $card) { version } }"#;
pub const UPLOAD: &str = r#"
mutation Upload($keyPackages: [String!]!) { uploadMlsKeyPackages(keyPackages: $keyPackages) { available } }"#;
pub const RELAY_INFO: &str = "query { relayInfo { serverId roles directSpaceId } }";
pub const CREATE: &str = r#"
mutation Create($input: CreateConversationInput!) { createConversation(input: $input) { id } }"#;
pub const REQUESTS: &str = r#"
query Requests($status: DmRequestStatus) {
  dmRequests(status: $status) {
    id fromPrincipalId hostServerId conversationId status card { payload signature }
    heldItems { id hostServerId workspaceId envelope { payload signature } }
  }
}"#;
pub const ITEMS: &str = r#"
query Items($after: ID) {
  relayItems(after: $after, limit: 500) {
    items { id hostServerId workspaceId envelope { payload signature } } hasMore
  }
}"#;
pub const CLAIM: &str = r#"
mutation Claim($input: ClaimMlsKeyPackagesInput!) {
  claimMlsKeyPackages(input: $input) { principalId deviceId keyPackage lastResort }
}"#;
pub const SUBMIT: &str = r#"
mutation Submit($input: SubmitMlsCommitInput!) { submitMlsCommit(input: $input) { epoch } }"#;
pub const WELCOMES: &str = "query { myMlsWelcomes { id conversationId epoch message } }";
pub const ACK_WELCOMES: &str = "mutation Ack($ids: [ID!]!) { ackMlsWelcomes(ids: $ids) }";
pub const SEND_ENCRYPTED: &str = r#"
mutation Send($input: SendEncryptedMessageInput!) { sendEncryptedMessage(input: $input) { id } }"#;
pub const EDIT_ENCRYPTED: &str = r#"
mutation Edit($input: EditEncryptedMessageInput!) { editEncryptedMessage(input: $input) { id } }"#;
pub const MARK_READ: &str = r#"
mutation Read($input: MarkAsReadInput!) { markAsRead(input: $input) }"#;
pub const HEADS: &str = r#"
query Heads($serverId: ID!, $streams: [String!]!) { streamHeads(serverId: $serverId, streams: $streams) { stream seq } }"#;

pub fn open_registration(config: &mut RIMConfig) {
    config.open_registration = Some(true);
}

pub async fn relays() -> (Node, Node) {
    let (a, b) = databases_with(RESET).await;
    (
        start(&a, open_registration).await,
        start(&b, open_registration).await,
    )
}

pub fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

pub fn signed(v: &Value) -> SignedStatement {
    SignedStatement {
        payload: v["payload"].as_str().expect("payload").to_string(),
        signature: v["signature"].as_str().expect("signature").to_string(),
    }
}

pub fn host_key(node: &Node) -> PublicKey {
    node.identity().state.server_key.clone()
}

/// Poll `check` until it gives a value, for up to 20 seconds.
pub async fn eventually<T, F, Fut>(what: &str, mut check: F) -> T
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
pub struct Item {
    pub id: i64,
    pub stream: String,
    pub seq: Option<u64>,
    pub event: Value,
}

/// An answer to a forwarded request, after its host response was checked.
pub struct Forwarded {
    pub status: u16,
    pub body: Value,
    pub bytes: Vec<u8>,
    pub host_signed: bool,
}

/// A person with one device, signed up on their relay over HTTP.
pub struct Person {
    pub origin: String,
    pub http: reqwest::Client,
    pub session: String,
    pub csrf: String,
    pub device_key: SigningKey,
    pub identity: TestIdentity,
    pub state: KeyState,
    pub mls: MlsDevice,
    pub seq: u64,
}

impl Person {
    pub fn principal(&self) -> String {
        self.mls.principal_id.clone()
    }

    pub fn device_id(&self) -> Uuid {
        self.mls.device_id
    }

    pub async fn post(&self, query: &str, variables: Value) -> Value {
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
    pub async fn ok(&self, query: &str, variables: Value) -> Value {
        let body = self.post(query, variables).await;
        assert!(body.get("errors").is_none(), "errors: {body}");
        body["data"].clone()
    }

    /// Run a query on the relay that must fail; returns the first error's code.
    pub async fn err(&self, query: &str, variables: Value) -> String {
        let body = self.post(query, variables).await;
        body["errors"][0]["extensions"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an error: {body}"))
            .to_string()
    }

    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Send a device-signed request through the relay to `host`. `sent_body` replaces the
    /// signed body on the wire; `seq` reuses a sequence number.
    pub async fn forward(
        &mut self,
        host: &Node,
        method: &str,
        path: &str,
        body: Vec<u8>,
        sent_body: Option<Vec<u8>>,
        seq: Option<u64>,
    ) -> (Forwarded, u64) {
        self.forward_typed(host, method, path, body, sent_body, seq, "application/json")
            .await
    }

    /// [`Person::forward`] with a `POST` body of `content_type`.
    #[allow(clippy::too_many_arguments)]
    pub async fn forward_typed(
        &mut self,
        host: &Node,
        method: &str,
        path: &str,
        body: Vec<u8>,
        sent_body: Option<Vec<u8>>,
        seq: Option<u64>,
        content_type: &str,
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
                .header("content-type", content_type)
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
                bytes: bytes.to_vec(),
                host_signed,
            },
            seq,
        )
    }

    /// Run GraphQL at `host` through the relay; panics unless it succeeded.
    pub async fn remote(&mut self, host: &Node, query: &str, variables: Value) -> Value {
        let body =
            serde_json::to_vec(&json!({ "query": query, "variables": variables })).expect("json");
        let (answer, _) = self.forward(host, "POST", "/api", body, None, None).await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert!(answer.host_signed);
        assert!(answer.body.get("errors").is_none(), "{}", answer.body);
        answer.body["data"].clone()
    }

    /// The relay's queued items, each envelope verified under `host`'s key.
    pub async fn items(&self, host: &Node, after: Option<i64>) -> Vec<Item> {
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
    pub async fn item_of(&self, host: &Node, kind: &str, matches: impl Fn(&Item) -> bool) -> Item {
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
pub fn verify_item(host: &Node, item: &Value) -> Item {
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

/// `node`'s direct space, read without a session, so nobody joins it.
pub async fn anonymous_direct_space(node: &Node) -> String {
    let body: Value = reqwest::Client::new()
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": RELAY_INFO }))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    body["data"]["relayInfo"]["directSpaceId"]
        .as_str()
        .expect("direct space")
        .to_string()
}

/// Sign up `name` on `node` with a key, publish a card naming the node, and upload key packages.
pub async fn sign_up(node: &Node, name: &str, seed: u8) -> Person {
    sign_up_with_card(node, name, seed, 1).await
}

/// The identity and device a seed stands for: one person with one device everywhere.
pub struct Keys {
    pub identity: TestIdentity,
    pub state: KeyState,
    pub device_id: Uuid,
    pub device_key: SigningKey,
}

pub fn keys(seed: u8) -> Keys {
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let state = KeyState::from_inception(&identity.inception()).expect("inception");
    Keys {
        identity,
        state,
        device_id: Uuid::from_u128(0x1000 + u128::from(seed)),
        device_key: signing_key(seed.wrapping_add(50)),
    }
}

/// A `login` statement by the seed's device over a fresh challenge from `node`.
pub async fn login_proof(http: &reqwest::Client, node: &Node, keys: &Keys) -> SignedStatement {
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
    device_login(
        &keys.device_key,
        &keys.state,
        keys.device_id,
        challenge["challengeId"]
            .as_str()
            .expect("id")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        &node.id(),
    )
}

/// Run `signupWithKey` for the seed on `node`, with `extra` input fields (a token or a
/// proof-of-work). Returns the answer and the cookies it set.
pub async fn key_signup(
    http: &reqwest::Client,
    node: &Node,
    name: &str,
    keys: &Keys,
    extra: Value,
) -> (Value, std::collections::HashMap<String, String>) {
    let cert = keys
        .identity
        .device_cert(&keys.state, keys.device_id, &keys.device_key, "Desktop");
    let proof = login_proof(http, node, keys).await;
    let mut input = json!({
        "username": name,
        "keyLog": [statement(&keys.identity.inception())],
        "device": statement(&cert),
        "proof": statement(&proof),
    });
    for (field, value) in extra.as_object().into_iter().flatten() {
        input[field] = value.clone();
    }
    let response = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": SIGNUP, "variables": { "input": input } }))
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
    (response.json().await.expect("json"), cookies)
}

/// [`sign_up`] publishing card `card_version`. A seed is one person with one device everywhere.
pub async fn sign_up_with_card(node: &Node, name: &str, seed: u8, card_version: u64) -> Person {
    let http = reqwest::Client::new();
    let Keys {
        identity,
        state,
        device_id,
        device_key,
    } = keys(seed);
    let (body, mut cookies) = key_signup(&http, node, name, &keys(seed), json!({})).await;
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
    pub async fn publish_card(&self, version: u64, relays: &[&str]) {
        let card = self.identity.profile_card(
            &self.state,
            version,
            &format!("Person {}", &self.principal()[6..10]),
            relays,
            &[(self.device_id(), &self.device_key)],
        );
        self.ok(PUBLISH, json!({ "card": statement(&card) })).await;
    }

    pub async fn direct_space(&self) -> String {
        self.ok(RELAY_INFO, json!({})).await["relayInfo"]["directSpaceId"]
            .as_str()
            .expect("direct space")
            .to_string()
    }

    /// Start a conversation in the direct space with someone on another relay.
    pub async fn start_dm(&self, other: &Person, other_relay: &Node, encrypted: bool) -> String {
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
    pub async fn create_group_with(&mut self, other: &Person, direct: &str, conv: &str) {
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
    pub async fn send_encrypted(&mut self, direct: &str, conv: &str, body: &str) -> Uuid {
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

    pub async fn pending_request(&self, from: &Person) -> Value {
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
pub async fn drained(host: &Node) {
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

pub async fn device_key_id(node: &Node, device_id: Uuid) -> DeviceKeyId {
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

pub async fn user_id(node: &Node, principal_id: &str) -> UserId {
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
