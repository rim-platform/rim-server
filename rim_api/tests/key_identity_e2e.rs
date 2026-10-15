//! Resolver-level e2e test for key identity and challenge login over a real database.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::Request;
use serde_json::{Value, json};
use uuid::Uuid;

use chrono::Utc;
use rim_api::auth_context::RemoteIp;
use rim_auth::password::hash_password;
use rim_core::auth::repository::{InstanceInviteRepository, SessionRepository};
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::{NewUser, UserId};
use rim_core::user::repository::UserRepository;
use rim_identity::testing::{TestIdentity, device_login, device_self_revoke, signing_key};
use rim_identity::{KeyState, PublicKey, SignedStatement, StatementVersion};
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Call, Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code,
    exec_ok, merged,
};
use support::identity::{
    Enrolled, SIGNUP_WITH_KEY, signed_login, signed_out, signup_input, statement, tampered,
};
use support::registration::seed_operator_invite;
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const PASSWORD: &str = "correct horse battery";

const ENROLL: &str = r#"
mutation Enroll($input: EnrollIdentityKeyInput!) {
  enrollIdentityKey(input: $input) {
    principalId
    keySeq
    keyLog { seq kind statement { payload signature } }
    devices { deviceId publicKey name keySeq revokedAt }
  }
}"#;

const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce expiresAt } }";

const LOGIN_WITH_KEY: &str = r#"
mutation Login($proof: SignedStatementInput!) { loginWithKey(proof: $proof) { userId } }"#;

const REVOKE: &str = r#"
mutation Revoke($revocation: SignedStatementInput!) {
  revokeDeviceKey(revocation: $revocation) { deviceId revokedAt }
}"#;

const MY_KEY_IDENTITY: &str = r#"
query { myKeyIdentity { principalId keySeq devices { deviceId revokedAt } } }"#;

const KEY_LOG: &str = r#"
query KeyLog($principalId: String!) { keyLog(principalId: $principalId) { seq kind statement { payload } } }"#;

const PASSWORD_LOGIN: &str = r#"
mutation Login($input: LoginInput!) { login(input: $input) { userId } }"#;

fn password_login(username: &str) -> Request {
    anonymous_request(
        PASSWORD_LOGIN,
        json!({ "input": { "username": username, "password": PASSWORD } }),
    )
}

/// Enroll a password account, sign in with the device key, revoke the device, and check that
/// the password and the revoked device no longer sign in. Every statement is version 1.
#[tokio::test]
async fn enroll_then_key_login_then_revoke() {
    enroll_login_revoke(StatementVersion::V1).await;
}

/// The same flow with version 2 statements: tagged keys and signatures.
#[tokio::test]
async fn enroll_then_key_login_then_revoke_v2() {
    enroll_login_revoke(StatementVersion::V2).await;
}

async fn enroll_login_revoke(version: StatementVersion) {
    let ctx = TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let user = users
        .create_user(NewUser {
            username: "ada".to_string(),
            password: Some(hash_password(PASSWORD).expect("hash")),
            email: Some("ada@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("create_user");
    let caller = Caller {
        user_id: user.id,
        session_id: ctx.create_session(&user).await,
    };
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // The password signs in before enrollment.
    exec_ok(&schema, password_login("ada")).await;

    let id = TestIdentity::with_version(1, version);
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(20);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");
    // The enrolling device proves it holds its key by signing a fresh challenge.
    let challenge = exec_ok(&schema, anonymous_request(CHALLENGE, json!({}))).await;
    let challenge = &challenge["keyLoginChallenge"];
    let proof = device_login(
        &device,
        &state,
        device_id,
        challenge["challengeId"]
            .as_str()
            .expect("challengeId")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        KEY_LOGIN_AUDIENCE,
    );

    let data = exec_ok(
        &schema,
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
    // Another account replaying the enrollment is refused: its challenge is spent.
    let eve = ctx.outsider("eve").await;
    let replay = eve.request(
        ENROLL,
        json!({ "input": {
            "keyLog": [statement(&inception)],
            "device": statement(&cert),
            "proof": statement(&proof),
        } }),
    );
    assert_eq!(exec_err_code(&schema, replay).await, "VALIDATION");
    let enrolled = &data["enrollIdentityKey"];
    assert_eq!(enrolled["principalId"], json!(state.principal_id));
    assert_eq!(enrolled["keySeq"], json!(0));
    assert_eq!(enrolled["keyLog"][0]["kind"], json!("INCEPTION"));
    assert_eq!(
        enrolled["keyLog"][0]["statement"]["payload"],
        json!(inception.payload)
    );
    assert_eq!(
        enrolled["devices"][0]["deviceId"],
        json!(device_id.to_string())
    );
    // The API returns the device key tagged, whichever version certified it.
    assert_eq!(
        enrolled["devices"][0]["publicKey"],
        json!(PublicKey::from(&device).to_string())
    );
    assert!(
        enrolled["devices"][0]["publicKey"]
            .as_str()
            .is_some_and(|k| k.starts_with("ed25519:"))
    );

    // The password no longer signs in.
    assert_eq!(
        exec_err_code(&schema, password_login("ada")).await,
        "UNAUTHENTICATED"
    );

    // The key log is public: a signed-out device reads it by principal id.
    let data = exec_ok(
        &schema,
        anonymous_request(KEY_LOG, json!({ "principalId": state.principal_id })),
    )
    .await;
    assert_eq!(data["keyLog"][0]["kind"], json!("INCEPTION"));
    assert_eq!(
        data["keyLog"][0]["statement"]["payload"],
        json!(inception.payload)
    );
    for nobody in ["rimp1_nobody".to_string(), "x".repeat(65)] {
        let data = exec_ok(
            &schema,
            anonymous_request(KEY_LOG, json!({ "principalId": nobody })),
        )
        .await;
        assert_eq!(data["keyLog"], json!([]));
    }

    // Challenge login.
    let login = |challenge: &Value| {
        let proof = device_login(
            &device,
            &state,
            device_id,
            challenge["challengeId"]
                .as_str()
                .expect("challengeId")
                .parse()
                .expect("uuid"),
            challenge["nonce"].as_str().expect("nonce"),
            KEY_LOGIN_AUDIENCE,
        );
        anonymous_request(LOGIN_WITH_KEY, json!({ "proof": statement(&proof) }))
            .data(RemoteIp::new("198.51.100.1"))
    };
    let challenge = exec_ok(&schema, anonymous_request(CHALLENGE, json!({}))).await;
    let data = exec_ok(&schema, login(&challenge["keyLoginChallenge"])).await;
    assert_eq!(data["loginWithKey"]["userId"], json!(user.id.to_string()));
    // The same challenge cannot be used twice.
    assert_eq!(
        exec_err_code(&schema, login(&challenge["keyLoginChallenge"])).await,
        "UNAUTHENTICATED"
    );

    let data = exec_ok(&schema, caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert_eq!(
        data["myKeyIdentity"]["devices"].as_array().map(Vec::len),
        Some(1)
    );

    // Revoke the device with the identity key: its sessions end, the enrolling one included.
    let revocation = id.revoke_device(&state, device_id);
    let data = exec_ok(
        &schema,
        caller.request(REVOKE, json!({ "revocation": statement(&revocation) })),
    )
    .await;
    assert!(data["revokeDeviceKey"]["revokedAt"].is_string());
    let enrolling = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_id(caller.session_id)
        .await
        .expect("get session")
        .expect("session");
    assert!(enrolling.revoked_at.is_some());

    // A revoked device's valid proof is refused.
    let challenge = exec_ok(&schema, anonymous_request(CHALLENGE, json!({}))).await;
    assert_eq!(
        exec_err_code(&schema, login(&challenge["keyLoginChallenge"])).await,
        "FORBIDDEN"
    );
}

/// `myKeyIdentity` is empty for an account without a key.
#[tokio::test]
async fn my_key_identity_before_enrollment() {
    let ctx = TestCtx::new().await;
    let caller = ctx.outsider("grace").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let data = exec_ok(&schema, caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert_eq!(data["myKeyIdentity"]["principalId"], Value::Null);
    assert_eq!(data["myKeyIdentity"]["devices"], json!([]));
}

const ROTATE: &str = r#"
mutation Rotate($input: RotateIdentityKeyInput!) {
  rotateIdentityKey(input: $input) { keySeq devices { deviceId revokedAt } }
}"#;

/// The roots that apply a statement to the caller's identity, each taking it as `$s`.
const SIGNED_ROOTS: [&str; 4] = [
    "mutation($s: SignedStatementInput!) { addDeviceKey(certificate: $s) { deviceId } }",
    "mutation($s: SignedStatementInput!) { publishProfileCard(card: $s) { version } }",
    "mutation($s: SignedStatementInput!) { rotateIdentityKey(input: { entry: $s }) { keySeq } }",
    "mutation($s: SignedStatementInput!) { revokeDeviceKey(revocation: $s) { revokedAt } }",
];

/// `who`'s statements for [`SIGNED_ROOTS`], in an order that applies: a certificate for a new
/// device, a profile card, an identity rotation and the first device retiring itself.
fn signed_by(who: &mut Enrolled) -> Vec<SignedStatement> {
    vec![
        who.identity
            .device_cert(&who.state, Uuid::now_v7(), &signing_key(200), "Phone"),
        who.identity
            .profile_card(&who.state, 1, "Ada", &[], &[(who.device_id, &who.device)]),
        who.identity.rotate_with_identity(&who.state, 201),
        device_self_revoke(&who.device, &who.state, who.device_id),
    ]
}

/// [`SIGNED_ROOTS`], each sent with `statements` changed by `change`.
fn signed_calls(
    statements: &[SignedStatement],
    change: impl Fn(&SignedStatement) -> SignedStatement,
) -> Vec<Call> {
    SIGNED_ROOTS
        .iter()
        .zip(statements)
        .map(|(query, s)| (*query, json!({ "s": statement(&change(s)) })))
        .collect()
}

/// Without a session every root on the caller's identity is refused, a rotation by the
/// identity key and one for a principal nobody here holds included. A statement whose signature
/// does not verify or whose payload is not a statement is `VALIDATION`. Another account
/// presenting someone's valid statements changes nothing of theirs. Applied once, the
/// statements do not apply again: the certificate, card and rotation name a head that moved.
#[tokio::test]
async fn signed_statements_apply_only_to_their_own_account_and_once() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let mut ada = ctx.enrolled(&schema, "ada", 6).await;
    let bob = ctx.enrolled(&schema, "bob", 7).await;
    let signed = signed_by(&mut ada);
    let valid = signed_calls(&signed, Clone::clone);

    let mut nobody = TestIdentity::with_version(8, StatementVersion::V2);
    let nobody_state = KeyState::from_inception(&nobody.inception()).expect("inception");
    // An enrollment of `inception` whose certificate and proof do not verify.
    let enroll = |inception: &SignedStatement| {
        let junk = statement(&tampered(inception));
        (
            ENROLL,
            json!({ "input": { "keyLog": [statement(inception)], "device": junk, "proof": junk } }),
        )
    };
    let mut anonymous = valid.clone();
    anonymous.extend([
        (MY_KEY_IDENTITY, json!({})),
        enroll(&ada.identity.inception()),
        (
            ROTATE,
            json!({ "input": { "entry": statement(&nobody.recover(&nobody_state, 9)) } }),
        ),
    ]);
    assert_each_refused(&schema, &anonymous, "UNAUTHENTICATED", signed_out).await;

    let mut bad = signed_calls(&signed, tampered);
    bad.extend(signed_calls(&signed, |s| SignedStatement {
        payload: "{}".to_string(),
        signature: s.signature.clone(),
    }));
    bad.push(enroll(&tampered(
        &TestIdentity::with_version(10, StatementVersion::V2).inception(),
    )));
    assert_each_refused_as(&schema, &ada.caller, &bad, "VALIDATION").await;

    for ((query, variables), code) in
        valid
            .iter()
            .cloned()
            .zip(["VALIDATION", "VALIDATION", "FORBIDDEN", "NOT_FOUND"])
    {
        let got = exec_err_code(&schema, bob.caller.request(query, variables)).await;
        assert_eq!(got, code, "{query}");
    }
    let mine = exec_ok(&schema, ada.caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert_eq!(mine["myKeyIdentity"]["keySeq"], json!(0));
    assert_eq!(
        mine["myKeyIdentity"]["devices"],
        json!([{ "deviceId": ada.device_id.to_string(), "revokedAt": null }])
    );

    for call in &valid {
        exec_ok(&schema, ada.caller.call(call.clone())).await;
    }
    assert_each_refused_as(&schema, &ada.caller, &valid[..3], "VALIDATION").await;
    let revoked_again = exec_ok(&schema, ada.caller.call(valid[3].clone())).await;
    assert!(revoked_again["revokeDeviceKey"]["revokedAt"].is_string());
}

/// An identity rotation keeps the devices. A card is stored once per version. A recovery
/// rotation needs no session and a device under the new key: it revokes every device, ends
/// every session and enrolls the new device, which signs in at once. A recovery whose device
/// reuses a revoked device's id changes nothing.
#[tokio::test]
async fn rotation_keeps_devices_and_recovery_replaces_them() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let mut ada = ctx.enrolled(&schema, "ada", 11).await;

    let rotation = ada.identity.rotate_with_identity(&ada.state, 212);
    let data = exec_ok(
        &schema,
        ada.caller.request(
            ROTATE,
            json!({ "input": { "entry": statement(&rotation) } }),
        ),
    )
    .await;
    assert_eq!(
        data["rotateIdentityKey"],
        json!({ "keySeq": 1, "devices": [{ "deviceId": ada.device_id.to_string(), "revokedAt": null }] })
    );
    ada.state = ada.state.apply(&rotation).expect("rotation").0;

    let card = ada.identity.profile_card(&ada.state, 1, "Ada", &[], &[]);
    let publish = || {
        ada.caller
            .request(SIGNED_ROOTS[1], json!({ "s": statement(&card) }))
    };
    exec_ok(&schema, publish()).await;
    assert_eq!(exec_err_code(&schema, publish()).await, "CONFLICT");

    let recovery = ada.identity.recover(&ada.state, 213);
    let recovered = ada.state.apply(&recovery).expect("recovery").0;
    let recover = |device: Option<SignedStatement>| {
        signed_out(
            ROTATE,
            json!({ "input": { "entry": statement(&recovery), "device": device.as_ref().map(statement) } }),
        )
    };
    assert_eq!(exec_err_code(&schema, recover(None)).await, "VALIDATION");
    let reused = ada
        .identity
        .device_cert(&recovered, ada.device_id, &signing_key(214), "Phone");
    assert_eq!(
        exec_err_code(&schema, recover(Some(reused))).await,
        "CONFLICT"
    );
    let mine = exec_ok(&schema, ada.caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert_eq!(mine["myKeyIdentity"]["keySeq"], json!(1));

    let phone_id = Uuid::now_v7();
    let phone = signing_key(215);
    let cert = ada
        .identity
        .device_cert(&recovered, phone_id, &phone, "Phone");
    let data = exec_ok(&schema, recover(Some(cert))).await;
    let devices = &data["rotateIdentityKey"]["devices"];
    assert_eq!(data["rotateIdentityKey"]["keySeq"], json!(2));
    assert!(devices[0]["revokedAt"].is_string());
    assert_eq!(
        devices[1],
        json!({ "deviceId": phone_id.to_string(), "revokedAt": null })
    );
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_id(ada.caller.session_id)
        .await
        .expect("get session")
        .expect("session");
    assert!(session.revoked_at.is_some());

    let login =
        |proof: &SignedStatement| signed_out(LOGIN_WITH_KEY, json!({ "proof": statement(proof) }));
    let proof = signed_login(&schema, &phone, &recovered, phone_id).await;
    let data = exec_ok(&schema, login(&proof)).await;
    assert_eq!(
        data["loginWithKey"]["userId"],
        json!(ada.user.id.to_string())
    );
    let proof = signed_login(&schema, &ada.device, &recovered, ada.device_id).await;
    assert_eq!(exec_err_code(&schema, login(&proof)).await, "FORBIDDEN");
}

/// `signupWithKey` with `input`, from a fixed address.
fn signup(input: &Value) -> Request {
    anonymous_request(SIGNUP_WITH_KEY, json!({ "input": input }))
        .data(RemoteIp::new("198.51.100.7"))
}

/// `signupWithKey` for `username`, with a new device of `id` and a fresh proof; `extra` adds
/// input fields (a token).
async fn signup_request(
    schema: &rim_api::AppSchema,
    username: &str,
    id: &TestIdentity,
    device_seed: u8,
    extra: Value,
) -> Request {
    signup(&signup_input(schema, username, id, device_seed, extra).await)
}

/// Open mode: an account is created from a version 2 key identity alone. It signs in with its
/// device key; no password signs in to it.
#[tokio::test]
async fn open_signup_with_key_then_key_login_and_no_password() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;

    let id = TestIdentity::with_version(3, StatementVersion::V2);
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(23);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");
    let proof = signed_login(&schema, &device, &state, device_id).await;
    let data = exec_ok(
        &schema,
        anonymous_request(
            SIGNUP_WITH_KEY,
            json!({ "input": {
                "username": "keyada",
                "displayName": "  Ada  ",
                "keyLog": [statement(&inception)],
                "device": statement(&cert),
                "proof": statement(&proof),
            } }),
        )
        .data(RemoteIp::new("198.51.100.7")),
    )
    .await;
    assert!(data["signupWithKey"]["serverId"].is_null());
    let user_id: UserId = data["signupWithKey"]["userId"]
        .as_str()
        .expect("userId")
        .parse::<Uuid>()
        .expect("uuid")
        .into();

    let user = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(user_id)
        .await
        .expect("get user")
        .expect("user");
    assert_eq!(user.password_hash, None);
    assert_eq!(user.email, None);
    assert_eq!(user.display_name.as_deref(), Some("Ada"));
    assert_eq!(
        user.principal_id.as_deref(),
        Some(state.principal_id.as_str())
    );

    // The device signs in with a new challenge.
    let proof = signed_login(&schema, &device, &state, device_id).await;
    let data = exec_ok(
        &schema,
        anonymous_request(LOGIN_WITH_KEY, json!({ "proof": statement(&proof) }))
            .data(RemoteIp::new("198.51.100.1")),
    )
    .await;
    assert_eq!(data["loginWithKey"]["userId"], json!(user_id.to_string()));

    // No password signs in to a key-only account.
    assert_eq!(
        exec_err_code(&schema, password_login("keyada")).await,
        "UNAUTHENTICATED"
    );
}

/// Invite-only mode with an operator's instance invite: the invite is spent and the account
/// starts with no workspace, as for `signupWithInvite`.
#[tokio::test]
async fn invite_only_signup_with_key_and_operator_invite() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let raw_token = "test_operator_invite_key_signup_abcdefgh";
    let invite_id = seed_operator_invite(
        &ctx.db,
        raw_token,
        Utc::now() + chrono::Duration::minutes(15),
    )
    .await;
    let id = TestIdentity::with_version(4, StatementVersion::V2);

    // Without a token, invite-only mode refuses.
    let request = signup_request(&schema, "admin", &id, 24, json!({})).await;
    assert_eq!(exec_err_code(&schema, request).await, "VALIDATION");

    let request = signup_request(
        &schema,
        "admin",
        &id,
        24,
        json!({ "instanceInviteToken": raw_token }),
    )
    .await;
    let data = exec_ok(&schema, request).await;
    assert!(data["signupWithKey"]["serverId"].is_null());
    let user_id: UserId = data["signupWithKey"]["userId"]
        .as_str()
        .expect("userId")
        .parse::<Uuid>()
        .expect("uuid")
        .into();

    let invite = SeaOrmInstanceInviteRepository::new(ctx.db.clone())
        .get_by_id(invite_id)
        .await
        .expect("get invite")
        .expect("invite");
    assert_eq!(invite.use_count, 1);

    let user = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(user_id)
        .await
        .expect("get user")
        .expect("user");
    assert_eq!(user.password_hash, None);
    assert_eq!(user.email, None);
    assert!(user.principal_id.is_some());

    let servers = SeaOrmServerRepository::new(ctx.db.clone())
        .get_servers_for_user(user_id)
        .await
        .expect("list memberships");
    assert!(
        servers.is_empty(),
        "an instance invite signup joins no workspace"
    );
}

/// A second account may not take a principal id another account here holds, and a signup's
/// proof does not sign up twice.
#[tokio::test]
async fn second_signup_with_the_same_principal_is_a_conflict() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let id = TestIdentity::with_version(5, StatementVersion::V2);

    let first = signup_input(&schema, "first", &id, 25, json!({})).await;
    exec_ok(&schema, signup(&first)).await;
    // The same identity, from another device, under another username.
    let request = signup_request(&schema, "second", &id, 26, json!({})).await;
    assert_eq!(exec_err_code(&schema, request).await, "CONFLICT");
    // The first signup again, under another username: its challenge is spent.
    let replay = merged(first, json!({ "username": "third" }));
    assert_eq!(exec_err_code(&schema, signup(&replay)).await, "VALIDATION");
}

/// A revoked device's proof answers `FORBIDDEN` only when its challenge and audience are
/// right; a proof for another server, over an unknown challenge, with a signature that does not
/// verify, a payload that is not a proof, from an unknown device or replayed answers
/// `UNAUTHENTICATED` like any other refusal.
#[tokio::test]
async fn finding_14_revocation_is_disclosed_only_to_a_proof_that_would_pass() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let lin = ctx.enrolled(&schema, "lin", 4).await;
    let revocation = lin.identity.revoke_device(&lin.state, lin.device_id);
    exec_ok(
        &schema,
        lin.caller
            .request(REVOKE, json!({ "revocation": statement(&revocation) })),
    )
    .await;

    let challenge = exec_ok(&schema, anonymous_request(CHALLENGE, json!({}))).await;
    let challenge = &challenge["keyLoginChallenge"];
    let challenge_id: Uuid = challenge["challengeId"]
        .as_str()
        .expect("challengeId")
        .parse()
        .expect("uuid");
    let nonce = challenge["nonce"].as_str().expect("nonce");
    let proof = |device_id: Uuid, challenge_id: Uuid, audience: &str| {
        device_login(
            &lin.device,
            &lin.state,
            device_id,
            challenge_id,
            nonce,
            audience,
        )
    };
    let login = |proof: Value| (LOGIN_WITH_KEY, json!({ "proof": proof }));
    let valid = proof(lin.device_id, challenge_id, KEY_LOGIN_AUDIENCE);
    let refused: Vec<Call> = vec![
        login(statement(&proof(
            lin.device_id,
            challenge_id,
            "evil.example.com",
        ))),
        login(statement(&proof(
            lin.device_id,
            Uuid::now_v7(),
            KEY_LOGIN_AUDIENCE,
        ))),
        login(statement(&proof(
            Uuid::now_v7(),
            challenge_id,
            KEY_LOGIN_AUDIENCE,
        ))),
        login(statement(&tampered(&valid))),
        login(json!({ "payload": "{\"v\":2}", "signature": valid.signature })),
    ];
    assert_each_refused(&schema, &refused, "UNAUTHENTICATED", signed_out).await;
    let (query, variables) = login(statement(&valid));
    let valid = || signed_out(query, variables.clone());
    assert_eq!(exec_err_code(&schema, valid()).await, "FORBIDDEN");
    assert_eq!(exec_err_code(&schema, valid()).await, "UNAUTHENTICATED");
}

/// The user signed up as `data`'s `signupWithKey`.
fn signed_up_user(data: &Value) -> UserId {
    data["signupWithKey"]["userId"]
        .as_str()
        .expect("userId")
        .parse::<Uuid>()
        .expect("uuid")
        .into()
}

/// The uses spent of the instance invite `id`.
async fn invite_uses(ctx: &TestCtx, id: rim_core::auth::model::InstanceInviteId) -> i32 {
    SeaOrmInstanceInviteRepository::new(ctx.db.clone())
        .get_by_id(id)
        .await
        .expect("get invite")
        .expect("invite")
        .use_count
}

/// An instance invite bound to an address signs up with a key and gives the account that
/// address. While an operator blocks the address, or another account holds it, the signup is
/// refused and the invite stays unspent.
#[tokio::test]
async fn signup_with_key_and_an_email_bound_invite_takes_its_address() {
    use sea_orm::ConnectionTrait;
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let issuer = ctx.create_user("issuer").await;
    let invite = seed_instance_invite(
        &ctx.db,
        issuer.id,
        "rim_iinv_bound_key",
        SeedInstanceInviteOpts {
            email: Some("ops@example.com".to_string()),
            ..Default::default()
        },
    )
    .await;
    let token = json!({ "instanceInviteToken": "rim_iinv_bound_key" });
    let id = TestIdentity::with_version(31, StatementVersion::V2);

    users
        .block_email("ops@example.com", "test")
        .await
        .expect("block");
    let request = signup_request(&schema, "ops", &id, 81, token.clone()).await;
    assert_eq!(exec_err_code(&schema, request).await, "VALIDATION");
    users
        .unblock_email("ops@example.com")
        .await
        .expect("unblock");

    let holder = users
        .create_user(NewUser {
            username: "holder".to_string(),
            password: Some(hash_password(PASSWORD).expect("hash")),
            email: Some("ops@example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("create holder");
    let request = signup_request(&schema, "ops", &id, 82, token.clone()).await;
    assert_eq!(exec_err_code(&schema, request).await, "EMAIL_TAKEN");
    assert!(users.find_by_username("ops").await.expect("find").is_none());
    assert_eq!(invite_uses(&ctx, invite).await, 0);

    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE \"user\" SET email = 'holder@example.com' WHERE id = '{}'",
            holder.id
        ))
        .await
        .expect("move the holder's address");
    let request = signup_request(&schema, "ops", &id, 83, token).await;
    let user_id = signed_up_user(&exec_ok(&schema, request).await);
    let user = users
        .get_by_id(user_id)
        .await
        .expect("get user")
        .expect("user");
    assert_eq!(user.email.as_deref(), Some("ops@example.com"));
    assert_eq!(user.password_hash, None);
    assert!(user.principal_id.is_some());
    assert_eq!(invite_uses(&ctx, invite).await, 1);
}

/// Two key signups racing for one single-use, email-bound invite create one account: the
/// loser's account, device and session roll back with its transaction.
#[tokio::test]
async fn racing_key_signups_for_one_email_bound_invite_create_one_account() {
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let issuer = ctx.create_user("issuer").await;
    let invite = seed_instance_invite(
        &ctx.db,
        issuer.id,
        "rim_iinv_bound_race",
        SeedInstanceInviteOpts {
            email: Some("race@example.com".to_string()),
            ..Default::default()
        },
    )
    .await;
    let token = json!({ "instanceInviteToken": "rim_iinv_bound_race" });
    let first = signup_request(
        &schema,
        "first",
        &TestIdentity::with_version(32, StatementVersion::V2),
        84,
        token.clone(),
    )
    .await;
    let second = signup_request(
        &schema,
        "second",
        &TestIdentity::with_version(33, StatementVersion::V2),
        85,
        token,
    )
    .await;

    let (first, second) = tokio::join!(schema.execute(first), schema.execute(second));
    let won: Vec<_> = [&first, &second]
        .into_iter()
        .filter(|r| r.errors.is_empty())
        .collect();
    assert_eq!(won.len(), 1, "{:?} / {:?}", first.errors, second.errors);
    assert_eq!(invite_uses(&ctx, invite).await, 1);

    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let holder = users
        .find_by_email("race@example.com")
        .await
        .expect("find")
        .expect("one account has the address");
    let winner = signed_up_user(&won[0].data.clone().into_json().expect("json"));
    assert_eq!(holder.id, winner);
    let accounts = [
        users.find_by_username("first").await.expect("find"),
        users.find_by_username("second").await.expect("find"),
    ];
    assert_eq!(accounts.iter().flatten().count(), 1);
}

/// Enrollment ends every other session of the account; the enrolling session, now the
/// device's, and another account's sessions go on.
#[tokio::test]
async fn enrollment_ends_every_other_session_of_the_account() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let sessions = SeaOrmSessionRepository::new(ctx.db.clone());
    let ada = ctx.create_user("ada").await;
    let others = [
        ctx.create_session(&ada).await,
        ctx.create_session(&ada).await,
    ];
    let bob = ctx.create_user("bob").await;
    let bobs = ctx.create_session(&bob).await;

    let enrolled = ctx.enroll_user(&schema, ada, 34).await;

    for id in others {
        let session = sessions.get_by_id(id).await.expect("get").expect("session");
        assert!(session.revoked_at.is_some(), "{id}");
    }
    let enrolling = sessions
        .get_by_id(enrolled.caller.session_id)
        .await
        .expect("get")
        .expect("session");
    assert!(enrolling.revoked_at.is_none());
    assert!(enrolling.device_key_id.is_some());
    let bobs = sessions
        .get_by_id(bobs)
        .await
        .expect("get")
        .expect("session");
    assert!(bobs.revoked_at.is_none());
    let data = exec_ok(&schema, enrolled.caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert!(data["myKeyIdentity"]["principalId"].is_string());
}
