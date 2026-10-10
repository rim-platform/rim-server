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
use rim_identity::testing::{SigningKey, TestIdentity, device_login, signing_key};
use rim_identity::{KeyState, PublicKey, SignedStatement, StatementVersion};
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
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

const SIGNUP_WITH_KEY: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId serverId } }"#;

const PASSWORD_LOGIN: &str = r#"
mutation Login($input: LoginInput!) { login(input: $input) { userId } }"#;

fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

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
    let data = exec_ok(
        &schema,
        anonymous_request(KEY_LOG, json!({ "principalId": "rimp1_nobody" })),
    )
    .await;
    assert_eq!(data["keyLog"], json!([]));

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

/// `myKeyIdentity` is empty for an account without a key, and needs a session.
#[tokio::test]
async fn my_key_identity_before_enrollment() {
    let ctx = TestCtx::new().await;
    let user = ctx.create_user("grace").await;
    let caller = Caller {
        user_id: user.id,
        session_id: ctx.create_session(&user).await,
    };
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let data = exec_ok(&schema, caller.request(MY_KEY_IDENTITY, json!({}))).await;
    assert_eq!(data["myKeyIdentity"]["principalId"], Value::Null);
    assert_eq!(data["myKeyIdentity"]["devices"], json!([]));

    assert_eq!(
        exec_err_code(&schema, anonymous_request(MY_KEY_IDENTITY, json!({}))).await,
        "UNAUTHENTICATED"
    );
}

/// A device's signed proof over a fresh challenge from `schema`.
async fn fresh_proof(
    schema: &rim_api::AppSchema,
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
) -> SignedStatement {
    let challenge = exec_ok(schema, anonymous_request(CHALLENGE, json!({}))).await;
    let challenge = &challenge["keyLoginChallenge"];
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

/// `signupWithKey` for `username`, with a new device of `id` and a fresh proof; `extra` adds
/// input fields (a token).
async fn signup_request(
    schema: &rim_api::AppSchema,
    username: &str,
    id: &TestIdentity,
    device_seed: u8,
    extra: Value,
) -> Request {
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(device_seed);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");
    let proof = fresh_proof(schema, &device, &state, device_id).await;
    let mut input = json!({
        "username": username,
        "keyLog": [statement(&inception)],
        "device": statement(&cert),
        "proof": statement(&proof),
    });
    if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        input.extend(extra.clone());
    }
    anonymous_request(SIGNUP_WITH_KEY, json!({ "input": input }))
        .data(RemoteIp::new("198.51.100.7"))
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
    let proof = fresh_proof(&schema, &device, &state, device_id).await;
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
    let proof = fresh_proof(&schema, &device, &state, device_id).await;
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

/// A second account may not take a principal id another account here holds.
#[tokio::test]
async fn second_signup_with_the_same_principal_is_a_conflict() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let id = TestIdentity::with_version(5, StatementVersion::V2);

    let request = signup_request(&schema, "first", &id, 25, json!({})).await;
    exec_ok(&schema, request).await;
    // The same identity, from another device, under another username.
    let request = signup_request(&schema, "second", &id, 26, json!({})).await;
    assert_eq!(exec_err_code(&schema, request).await, "CONFLICT");
}

/// A revoked device's proof answers `FORBIDDEN` only when its challenge and audience are
/// right; a proof for another server, over an unknown challenge or replayed answers
/// `UNAUTHENTICATED` like any other refusal.
#[tokio::test]
async fn finding_14_revocation_is_disclosed_only_to_a_proof_that_would_pass() {
    let ctx = TestCtx::new().await;
    let user = ctx.create_user("lin").await;
    let caller = Caller {
        user_id: user.id,
        session_id: ctx.create_session(&user).await,
    };
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let id = TestIdentity::with_version(4, StatementVersion::V2);
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(24);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");
    let proof = fresh_proof(&schema, &device, &state, device_id).await;
    exec_ok(
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
    let revocation = id.revoke_device(&state, device_id);
    exec_ok(
        &schema,
        caller.request(REVOKE, json!({ "revocation": statement(&revocation) })),
    )
    .await;

    let login = |proof: &SignedStatement| {
        anonymous_request(LOGIN_WITH_KEY, json!({ "proof": statement(proof) }))
            .data(RemoteIp::new("198.51.100.4"))
    };
    let challenge = exec_ok(&schema, anonymous_request(CHALLENGE, json!({}))).await;
    let challenge = &challenge["keyLoginChallenge"];
    let challenge_id: Uuid = challenge["challengeId"]
        .as_str()
        .expect("challengeId")
        .parse()
        .expect("uuid");
    let nonce = challenge["nonce"].as_str().expect("nonce");

    let other_server = device_login(
        &device,
        &state,
        device_id,
        challenge_id,
        nonce,
        "evil.example.com",
    );
    assert_eq!(
        exec_err_code(&schema, login(&other_server)).await,
        "UNAUTHENTICATED"
    );
    let unknown_challenge = device_login(
        &device,
        &state,
        device_id,
        Uuid::now_v7(),
        nonce,
        KEY_LOGIN_AUDIENCE,
    );
    assert_eq!(
        exec_err_code(&schema, login(&unknown_challenge)).await,
        "UNAUTHENTICATED"
    );
    let valid = device_login(
        &device,
        &state,
        device_id,
        challenge_id,
        nonce,
        KEY_LOGIN_AUDIENCE,
    );
    assert_eq!(exec_err_code(&schema, login(&valid)).await, "FORBIDDEN");
    assert_eq!(
        exec_err_code(&schema, login(&valid)).await,
        "UNAUTHENTICATED"
    );
}
