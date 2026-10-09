//! Resolver-level e2e test for device linking over a real database, with real sealed messages
//! from `rim_identity::device_link` (`docs/specs/device-linking-v1.md`).
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_api::auth_context::RemoteIp;
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
use rim_auth::session_token::hash_token;
use rim_core::auth::repository::SessionRepository;
use rim_core::user::model::User;
use rim_identity::device_link::{
    Answer, LinkCode, LinkId, LinkSecret, Offer, Platform, SealedMessage,
};
use rim_identity::testing::{SigningKey, TestIdentity, device_login, signing_key};
use rim_identity::{KeyState, PublicKey, SignedStatement, StatementVersion};
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_services::auth::dto::RegistrationMode;
use sea_orm::ConnectionTrait;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const ENDPOINT: &str = "http://localhost:8080/api";

const ENROLL: &str = r#"
mutation Enroll($input: EnrollIdentityKeyInput!) { enrollIdentityKey(input: $input) { principalId } }"#;
const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";
const LOGIN_WITH_KEY: &str = r#"
mutation Login($proof: SignedStatementInput!) { loginWithKey(proof: $proof) { userId } }"#;
const ADD_DEVICE: &str = r#"
mutation Add($certificate: SignedStatementInput!) { addDeviceKey(certificate: $certificate) { deviceId } }"#;
const KEY_LOG: &str = r#"
query KeyLog($principalId: String!) { keyLog(principalId: $principalId) { statement { payload signature } } }"#;
const MY_DEVICES: &str = "query { myKeyIdentity { devices { deviceId lastUsedAt } } }";

const START: &str = "mutation { startDeviceLink { linkId expiresAt } }";
const SEND_OFFER: &str = r#"
mutation Offer($linkId: String!, $offer: SealedMessageInput!) {
  sendDeviceLinkOffer(linkId: $linkId, offer: $offer)
}"#;
const DEVICE_LINK: &str = r#"
query Link($linkId: String!) { deviceLink(linkId: $linkId) { state expiresAt offer { nonce ciphertext } } }"#;
const SEND_ANSWER: &str = r#"
mutation Answer($linkId: String!, $answer: SealedMessageInput!) {
  sendDeviceLinkAnswer(linkId: $linkId, answer: $answer) { state expiresAt offer { nonce ciphertext } }
}"#;
const ANSWER: &str = r#"
query Answer($linkId: String!) { deviceLinkAnswer(linkId: $linkId) { nonce ciphertext } }"#;
const COMPLETE: &str = r#"
mutation Complete($linkId: String!) { completeDeviceLink(linkId: $linkId) }"#;
const CANCEL: &str = r#"
mutation Cancel($linkId: String!) { cancelDeviceLink(linkId: $linkId) }"#;

fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

fn sealed_json(sealed: &SealedMessage) -> Value {
    json!({ "nonce": sealed.nonce_text(), "ciphertext": sealed.ciphertext_text() })
}

fn sealed_from(value: &Value) -> SealedMessage {
    SealedMessage::from_text(
        value["nonce"].as_str().expect("nonce"),
        value["ciphertext"].as_str().expect("ciphertext"),
    )
    .expect("sealed message")
}

fn raw_sealed(nonce: &[u8], ciphertext: &[u8]) -> Value {
    json!({ "nonce": URL_SAFE_NO_PAD.encode(nonce), "ciphertext": URL_SAFE_NO_PAD.encode(ciphertext) })
}

/// A request from a device without a session, from `ip`.
fn anonymous(query: &str, variables: Value, ip: &str) -> Request {
    anonymous_request(query, variables).data(RemoteIp::new(ip))
}

/// Execute `request` and return its errors as JSON, which must not be empty.
async fn error_body(schema: &AppSchema, request: Request) -> Value {
    let res = schema.execute(request).await;
    assert!(
        !res.errors.is_empty(),
        "expected an error, got {:?}",
        res.data
    );
    serde_json::to_value(&res.errors).expect("errors serialize")
}

/// An account with a key identity (version 2) and its first device, as the old device holds it.
struct OldDevice {
    user: User,
    caller: Caller,
    identity: TestIdentity,
    state: KeyState,
}

async fn enrolled(ctx: &TestCtx, schema: &AppSchema, username: &str, seed: u8) -> OldDevice {
    let user = ctx.create_user(username).await;
    let caller = Caller {
        user_id: user.id,
        session_id: ctx.create_session(&user).await,
    };
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let inception = identity.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(seed.wrapping_add(50));
    let cert = identity.device_cert(&state, device_id, &device, "Desktop");
    let proof = signed_login(schema, &device, &state, device_id).await;
    exec_ok(
        schema,
        caller.request(
            ENROLL,
            json!({ "input": {
                "keyLog": [statement(&inception)],
                "device": statement(&cert),
                "proof": statement(&proof),
            } }),
        ),
    )
    .await;
    OldDevice {
        user,
        caller,
        identity,
        state,
    }
}

/// A login proof by `device` over a fresh challenge.
async fn signed_login(
    schema: &AppSchema,
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
) -> SignedStatement {
    let data = exec_ok(schema, anonymous(CHALLENGE, json!({}), "192.0.2.1")).await;
    let challenge = &data["keyLoginChallenge"];
    device_login(
        device,
        state,
        device_id,
        challenge["challengeId"]
            .as_str()
            .expect("challengeId")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        KEY_LOGIN_AUDIENCE,
    )
}

async fn start(schema: &AppSchema, caller: &Caller) -> String {
    let data = exec_ok(schema, caller.request(START, json!({}))).await;
    data["startDeviceLink"]["linkId"]
        .as_str()
        .expect("linkId")
        .to_string()
}

/// The whole flow of the spec: the old device starts a link and shows the code; the new device
/// seals an offer; the old device opens it, certifies the key and answers; the new device
/// verifies the certificate against the served key log, signs in with its own key and
/// completes the link; the old device sees `CONSUMED`.
#[tokio::test]
async fn a_new_device_links_signs_in_and_completes() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let old = enrolled(&ctx, &schema, "ada", 1).await;

    // Old device: start the link and show the code.
    let data = exec_ok(&schema, old.caller.request(START, json!({}))).await;
    let link_id = data["startDeviceLink"]["linkId"]
        .as_str()
        .expect("linkId")
        .to_string();
    assert!(data["startDeviceLink"]["expiresAt"].is_string());
    let secret = LinkSecret::generate();
    let shown = LinkCode::new(
        LinkId::parse(&link_id).expect("link id"),
        secret.clone(),
        ENDPOINT,
    )
    .expect("code");
    let old_channel = shown.channel();

    // New device: read the pasted code, make a key in memory, seal and send the offer.
    let pasted = LinkCode::parse(&format!("{}\n", shown)).expect("pasted code");
    assert_eq!(pasted.endpoint, ENDPOINT);
    let new_channel = pasted.channel();
    let new_device = signing_key(30);
    let offer = Offer {
        device_id: Uuid::now_v7(),
        device_key: PublicKey::from(&new_device),
        name: "Phone".to_string(),
        platform: Platform::Desktop,
        statement_version: 2,
    };
    let sealed_offer = new_channel.seal_offer(&offer).expect("seal offer");
    let send_offer = || {
        anonymous(
            SEND_OFFER,
            json!({ "linkId": pasted.link_id.to_string(), "offer": sealed_json(&sealed_offer.sealed) }),
            "198.51.100.20",
        )
    };
    let data = exec_ok(&schema, send_offer()).await;
    assert_eq!(data["sendDeviceLinkOffer"], json!(true));
    // A retry with the same bytes (its answer was lost) succeeds.
    exec_ok(&schema, send_offer()).await;
    // Nothing to fetch yet.
    let poll = || {
        anonymous(
            ANSWER,
            json!({ "linkId": pasted.link_id.to_string() }),
            "198.51.100.20",
        )
    };
    assert_eq!(
        exec_ok(&schema, poll()).await["deviceLinkAnswer"],
        Value::Null
    );

    // Old device: the offer arrived; open it and compare fingerprints.
    let data = exec_ok(
        &schema,
        old.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id })),
    )
    .await;
    assert_eq!(data["deviceLink"]["state"], json!("OFFERED"));
    let opened = old_channel
        .open_offer(&sealed_from(&data["deviceLink"]["offer"]))
        .expect("open offer");
    assert_eq!(opened.offer, offer);
    assert_eq!(opened.fingerprint, sealed_offer.fingerprint);

    // The person confirmed: certify the key, add the device, seal and send the answer.
    let certificate = old.identity.device_cert(
        &old.state,
        opened.offer.device_id,
        &new_device,
        &opened.offer.name,
    );
    exec_ok(
        &schema,
        old.caller.request(
            ADD_DEVICE,
            json!({ "certificate": statement(&certificate) }),
        ),
    )
    .await;
    let sealed_answer = old_channel
        .seal_answer(&Answer { certificate })
        .expect("seal answer");
    let data = exec_ok(
        &schema,
        old.caller.request(
            SEND_ANSWER,
            json!({ "linkId": link_id, "answer": sealed_json(&sealed_answer) }),
        ),
    )
    .await;
    assert_eq!(data["sendDeviceLinkAnswer"]["state"], json!("ANSWERED"));

    // New device: fetch the answer, verify the certificate against the served key log.
    let data = exec_ok(&schema, poll()).await;
    let answer = new_channel
        .open_answer(&sealed_from(&data["deviceLinkAnswer"]))
        .expect("open answer");
    let principal_id = old.state.principal_id.clone();
    let data = exec_ok(
        &schema,
        anonymous(
            KEY_LOG,
            json!({ "principalId": principal_id }),
            "198.51.100.20",
        ),
    )
    .await;
    let log: Vec<SignedStatement> = data["keyLog"]
        .as_array()
        .expect("key log")
        .iter()
        .map(|e| SignedStatement {
            payload: e["statement"]["payload"]
                .as_str()
                .expect("payload")
                .to_string(),
            signature: e["statement"]["signature"]
                .as_str()
                .expect("signature")
                .to_string(),
        })
        .collect();
    let served = KeyState::replay(&log).expect("replay served log");
    let cert = answer
        .verify(&offer, &served)
        .expect("certificate verifies");
    assert_eq!(cert.principal_id, principal_id);

    // New device: sign in with its own key.
    let proof = signed_login(&schema, &new_device, &served, offer.device_id).await;
    let intent = Arc::new(AuthIntentCtx::default());
    let login = anonymous(
        LOGIN_WITH_KEY,
        json!({ "proof": statement(&proof) }),
        "198.51.100.20",
    )
    .data(Arc::clone(&intent));
    let data = exec_ok(&schema, login).await;
    assert_eq!(
        data["loginWithKey"]["userId"],
        json!(old.user.id.to_string())
    );
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("loginWithKey must set a session");
    };
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_token_hash(&hash_token(&session_token))
        .await
        .expect("session lookup")
        .expect("session");
    let new_caller = Caller {
        user_id: old.user.id,
        session_id: session.id,
    };

    // New device: complete; a repeat answers `true` again.
    let complete = || new_caller.request(COMPLETE, json!({ "linkId": link_id }));
    assert_eq!(
        exec_ok(&schema, complete()).await["completeDeviceLink"],
        json!(true)
    );
    exec_ok(&schema, complete()).await;

    // Old device: `CONSUMED`, no sealed bytes left, and the new device was used.
    let data = exec_ok(
        &schema,
        old.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id })),
    )
    .await;
    assert_eq!(data["deviceLink"]["state"], json!("CONSUMED"));
    assert_eq!(data["deviceLink"]["offer"], Value::Null);
    let data = exec_ok(&schema, old.caller.request(MY_DEVICES, json!({}))).await;
    let devices = data["myKeyIdentity"]["devices"]
        .as_array()
        .expect("devices");
    let linked = devices
        .iter()
        .find(|d| d["deviceId"] == json!(offer.device_id.to_string()))
        .expect("linked device");
    assert!(linked["lastUsedAt"].is_string());

    // The link id no longer works for anyone without a session.
    assert_eq!(exec_err_code(&schema, poll()).await, "UNAUTHENTICATED");
}

/// A query, its variables for a link id, and the caller's IP.
type AnonymousCall = (&'static str, fn(&str) -> Value, &'static str);

/// Every anonymous call on a link it cannot act on answers the same `UNAUTHENTICATED` body,
/// whatever the reason, so a link id holder learns nothing about the link's state.
#[tokio::test]
async fn anonymous_refusals_are_identical_for_every_reason() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;

    let unknown = URL_SAFE_NO_PAD.encode([0xab; 16]);
    let malformed = "AAECAwQFBgcICQoLDA0ODw==".to_string();

    let cancelled = start(&schema, &ada.caller).await;
    exec_ok(
        &schema,
        ada.caller.request(CANCEL, json!({ "linkId": cancelled })),
    )
    .await;

    let consumed = start(&schema, &ada.caller).await;
    exec_ok(
        &schema,
        anonymous(
            SEND_OFFER,
            json!({ "linkId": consumed, "offer": raw_sealed(&[1; 12], &[1; 64]) }),
            "203.0.113.1",
        ),
    )
    .await;
    exec_ok(
        &schema,
        ada.caller.request(
            SEND_ANSWER,
            json!({ "linkId": consumed, "answer": raw_sealed(&[2; 12], &[2; 64]) }),
        ),
    )
    .await;
    exec_ok(
        &schema,
        ada.caller.request(COMPLETE, json!({ "linkId": consumed })),
    )
    .await;

    let expired = start(&schema, &ada.caller).await;
    ctx.conn
        .execute_unprepared(
            "UPDATE device_link SET expires_at = now() - interval '1 second' \
             WHERE state = 'open'",
        )
        .await
        .expect("expire the link");

    let links = [&unknown, &malformed, &cancelled, &consumed, &expired];
    let calls: [AnonymousCall; 3] = [
        (
            SEND_OFFER,
            |link_id| json!({ "linkId": link_id, "offer": raw_sealed(&[3; 12], &[3; 64]) }),
            "203.0.113.2",
        ),
        (
            ANSWER,
            |link_id| json!({ "linkId": link_id }),
            "203.0.113.3",
        ),
        (
            CANCEL,
            |link_id| json!({ "linkId": link_id }),
            "203.0.113.4",
        ),
    ];
    for (query, variables, ip) in calls {
        let mut bodies = Vec::new();
        for link_id in links {
            let body = error_body(&schema, anonymous(query, variables(link_id), ip)).await;
            assert_eq!(body[0]["extensions"]["code"], json!("UNAUTHENTICATED"));
            bodies.push(body);
        }
        assert!(
            bodies.windows(2).all(|w| w[0] == w[1]),
            "{query}: refusal bodies differ: {bodies:?}"
        );
    }

    // A session of another account is just another holder of the link id.
    let bob = enrolled(&ctx, &schema, "bob", 2).await;
    assert_eq!(
        exec_err_code(
            &schema,
            bob.caller.request(CANCEL, json!({ "linkId": cancelled })),
        )
        .await,
        "UNAUTHENTICATED"
    );
}

#[tokio::test]
async fn session_calls_on_another_users_link_are_not_found() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;
    let bob = enrolled(&ctx, &schema, "bob", 2).await;
    let link_id = start(&schema, &ada.caller).await;
    exec_ok(
        &schema,
        anonymous(
            SEND_OFFER,
            json!({ "linkId": link_id, "offer": raw_sealed(&[1; 12], &[1; 64]) }),
            "203.0.113.1",
        ),
    )
    .await;

    for request in [
        bob.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id })),
        bob.caller.request(
            SEND_ANSWER,
            json!({ "linkId": link_id, "answer": raw_sealed(&[2; 12], &[2; 64]) }),
        ),
        bob.caller.request(COMPLETE, json!({ "linkId": link_id })),
        ada.caller.request(
            DEVICE_LINK,
            json!({ "linkId": URL_SAFE_NO_PAD.encode([0xcd; 16]) }),
        ),
        ada.caller
            .request(DEVICE_LINK, json!({ "linkId": "not-a-link-id" })),
    ] {
        assert_eq!(exec_err_code(&schema, request).await, "NOT_FOUND");
    }

    // Without a session, the session calls are unauthenticated.
    for (query, variables) in [
        (START, json!({})),
        (DEVICE_LINK, json!({ "linkId": link_id })),
        (COMPLETE, json!({ "linkId": link_id })),
        (
            SEND_ANSWER,
            json!({ "linkId": link_id, "answer": raw_sealed(&[2; 12], &[2; 64]) }),
        ),
    ] {
        assert_eq!(
            exec_err_code(&schema, anonymous(query, variables, "203.0.113.9")).await,
            "UNAUTHENTICATED",
            "{query}"
        );
    }
}

#[tokio::test]
async fn an_answer_before_the_offer_is_a_conflict() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;
    let link_id = start(&schema, &ada.caller).await;
    assert_eq!(
        exec_err_code(
            &schema,
            ada.caller.request(
                SEND_ANSWER,
                json!({ "linkId": link_id, "answer": raw_sealed(&[2; 12], &[2; 64]) }),
            ),
        )
        .await,
        "CONFLICT"
    );
    assert_eq!(
        exec_err_code(
            &schema,
            ada.caller.request(COMPLETE, json!({ "linkId": link_id })),
        )
        .await,
        "CONFLICT"
    );
    let data = exec_ok(
        &schema,
        ada.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id })),
    )
    .await;
    assert_eq!(data["deviceLink"]["state"], json!("OPEN"));
}

/// The server never interprets the sealed bytes: random bytes go in and come out unchanged.
#[tokio::test]
async fn random_sealed_bytes_are_stored_and_returned_byte_exact() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;
    let link_id = start(&schema, &ada.caller).await;

    let random = |len: usize| -> Vec<u8> {
        let mut bytes = Vec::with_capacity(len);
        while bytes.len() < len {
            bytes.extend_from_slice(Uuid::new_v4().as_bytes());
        }
        bytes.truncate(len);
        bytes
    };
    let offer = raw_sealed(&random(12), &random(16_400));
    exec_ok(
        &schema,
        anonymous(
            SEND_OFFER,
            json!({ "linkId": link_id, "offer": offer }),
            "203.0.113.1",
        ),
    )
    .await;
    let data = exec_ok(
        &schema,
        ada.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id })),
    )
    .await;
    assert_eq!(data["deviceLink"]["offer"], offer);

    let answer = raw_sealed(&random(12), &random(65_552));
    let data = exec_ok(
        &schema,
        ada.caller
            .request(SEND_ANSWER, json!({ "linkId": link_id, "answer": answer })),
    )
    .await;
    assert_eq!(data["sendDeviceLinkAnswer"]["offer"], offer);
    let data = exec_ok(
        &schema,
        anonymous(ANSWER, json!({ "linkId": link_id }), "203.0.113.1"),
    )
    .await;
    assert_eq!(data["deviceLinkAnswer"], answer);
}

#[tokio::test]
async fn malformed_or_oversized_sealed_messages_are_validation_errors() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;
    let link_id = start(&schema, &ada.caller).await;

    let padded_nonce = json!({
        "nonce": format!("{}=", URL_SAFE_NO_PAD.encode([1; 12])),
        "ciphertext": URL_SAFE_NO_PAD.encode([1; 64]),
    });
    let standard_alphabet = json!({
        "nonce": URL_SAFE_NO_PAD.encode([1; 12]),
        "ciphertext": "++++".repeat(20),
    });
    for offer in [
        raw_sealed(&[1; 11], &[1; 64]),
        raw_sealed(&[1; 12], &[1; 16]),
        raw_sealed(&[1; 12], &vec![1; 16_401]),
        padded_nonce.clone(),
        standard_alphabet.clone(),
    ] {
        // Checked before the link is looked up, so a bad link id does not change the answer.
        for target in [link_id.as_str(), "not-a-link-id"] {
            assert_eq!(
                exec_err_code(
                    &schema,
                    anonymous(
                        SEND_OFFER,
                        json!({ "linkId": target, "offer": offer }),
                        "203.0.113.1",
                    ),
                )
                .await,
                "VALIDATION",
                "{offer}"
            );
        }
    }

    exec_ok(
        &schema,
        anonymous(
            SEND_OFFER,
            json!({ "linkId": link_id, "offer": raw_sealed(&[1; 12], &[1; 17]) }),
            "203.0.113.1",
        ),
    )
    .await;
    for answer in [
        raw_sealed(&[1; 13], &[1; 64]),
        raw_sealed(&[1; 12], &[]),
        raw_sealed(&[1; 12], &vec![1; 65_553]),
        padded_nonce,
        standard_alphabet,
    ] {
        assert_eq!(
            exec_err_code(
                &schema,
                ada.caller
                    .request(SEND_ANSWER, json!({ "linkId": link_id, "answer": answer }),),
            )
            .await,
            "VALIDATION",
            "{answer}"
        );
    }
}

#[tokio::test]
async fn start_needs_a_key_identity() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let grace = ctx.create_user("grace").await;
    let caller = Caller {
        user_id: grace.id,
        session_id: ctx.create_session(&grace).await,
    };
    let res = schema.execute(caller.request(START, json!({}))).await;
    let err = res.errors.first().expect("error");
    assert_eq!(err.message, "this account has no key identity yet");
    assert_eq!(
        err.extensions.as_ref().and_then(|e| e.get("code")),
        Some(&async_graphql::Value::from("VALIDATION"))
    );
}

/// The link's user may cancel at any point before `CONSUMED`, and again after; a second start
/// cancels the first link.
#[tokio::test]
async fn the_links_user_cancels_and_a_second_start_replaces_the_first() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let ada = enrolled(&ctx, &schema, "ada", 1).await;

    let first = start(&schema, &ada.caller).await;
    let second = start(&schema, &ada.caller).await;
    let state = |link_id: &str| {
        ada.caller
            .request(DEVICE_LINK, json!({ "linkId": link_id }))
    };
    assert_eq!(
        exec_ok(&schema, state(&first)).await["deviceLink"]["state"],
        json!("CANCELLED")
    );
    assert_eq!(
        exec_ok(&schema, state(&second)).await["deviceLink"]["state"],
        json!("OPEN")
    );

    let cancel = || ada.caller.request(CANCEL, json!({ "linkId": second }));
    assert_eq!(
        exec_ok(&schema, cancel()).await["cancelDeviceLink"],
        json!(true)
    );
    exec_ok(&schema, cancel()).await;
    assert_eq!(
        exec_ok(&schema, state(&second)).await["deviceLink"]["state"],
        json!("CANCELLED")
    );
    assert_eq!(
        exec_err_code(
            &schema,
            anonymous(
                SEND_OFFER,
                json!({ "linkId": second, "offer": raw_sealed(&[1; 12], &[1; 64]) }),
                "203.0.113.1"
            ),
        )
        .await,
        "UNAUTHENTICATED"
    );

    // The new device, without a session, cancels a live link.
    let third = start(&schema, &ada.caller).await;
    let data = exec_ok(
        &schema,
        anonymous(CANCEL, json!({ "linkId": third }), "203.0.113.1"),
    )
    .await;
    assert_eq!(data["cancelDeviceLink"], json!(true));
    assert_eq!(
        exec_ok(&schema, state(&third)).await["deviceLink"]["state"],
        json!("CANCELLED")
    );
}
