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

use rim_api::auth_context::RemoteIp;
use rim_auth::password::hash_password;
use rim_core::auth::repository::SessionRepository;
use rim_core::user::model::NewUser;
use rim_core::user::repository::UserRepository;
use rim_identity::testing::{TestIdentity, device_login, signing_key};
use rim_identity::{KeyState, SignedStatement};
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const PASSWORD: &str = "correct horse battery";

const ENROLL: &str = r#"
mutation Enroll($input: EnrollIdentityKeyInput!) {
  enrollIdentityKey(input: $input) {
    principalId
    keySeq
    keyLog { seq kind statement { payload signature } }
    devices { deviceId name keySeq revokedAt }
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
/// the password and the revoked device no longer sign in.
#[tokio::test]
async fn enroll_then_key_login_then_revoke() {
    let ctx = TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let user = users
        .create_user(NewUser {
            username: "ada".to_string(),
            password: hash_password(PASSWORD).expect("hash"),
            email: "ada@test.example.com".to_string(),
            display_name: None,
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

    let id = TestIdentity::new(1);
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(20);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");

    let data = exec_ok(
        &schema,
        caller.request(
            ENROLL,
            json!({ "input": { "keyLog": [statement(&inception)], "device": statement(&cert) } }),
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

    // The password no longer signs in.
    assert_eq!(
        exec_err_code(&schema, password_login("ada")).await,
        "UNAUTHENTICATED"
    );

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
