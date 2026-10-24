/// An account enrolled with a key identity, and the statement helpers the key identity, device
/// link and encryption suites share.
use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_api::auth_context::RemoteIp;
use rim_core::user::model::User;
use rim_identity::testing::{SigningKey, TestIdentity, device_login, signing_key};
use rim_identity::{KeyState, SignedStatement, StatementVersion};

use super::graphql::{Caller, anonymous_request, exec_ok};
use super::password::{
    LOGIN_WITH_PASSWORD, PasswordLogin, TEST_PASSWORD, b64, login, registration,
};
use super::{KEY_LOGIN_AUDIENCE, TestCtx};

const ENROLL: &str = r#"
mutation Enroll($input: EnrollIdentityKeyInput!) { enrollIdentityKey(input: $input) { principalId } }"#;
const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";

pub const SIGNUP_WITH_KEY: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId serverId } }"#;

/// A statement as GraphQL input.
pub fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

/// `s` with one bit of its signature flipped: well-formed, and it does not verify.
pub fn tampered(s: &SignedStatement) -> SignedStatement {
    let (tag, encoded) = match s.signature.split_once(':') {
        Some((tag, encoded)) => (format!("{tag}:"), encoded),
        None => (String::new(), s.signature.as_str()),
    };
    let mut bytes = URL_SAFE_NO_PAD.decode(encoded).expect("signature");
    bytes[0] ^= 1;
    SignedStatement {
        payload: s.payload.clone(),
        signature: format!("{tag}{}", URL_SAFE_NO_PAD.encode(bytes)),
    }
}

/// A request without a session from a fixed address.
pub fn signed_out(query: &str, variables: Value) -> Request {
    anonymous_request(query, variables).data(RemoteIp::new("192.0.2.1"))
}

/// A login proof by `device` over a fresh challenge.
pub async fn signed_login(
    schema: &AppSchema,
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
) -> SignedStatement {
    let data = exec_ok(schema, signed_out(CHALLENGE, json!({}))).await;
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

/// An account with a version 2 key identity, signed in on its first device.
pub struct Enrolled {
    pub user: User,
    /// The password session that enrolled, now bound to `device_id`.
    pub caller: Caller,
    pub identity: TestIdentity,
    pub state: KeyState,
    pub device_id: Uuid,
    pub device: SigningKey,
}

impl TestCtx {
    /// A user `name` who enrolled the identity from `seed` with the device key from `seed + 50`,
    /// named "Desktop".
    pub async fn enrolled(&self, schema: &AppSchema, name: &str, seed: u8) -> Enrolled {
        let user = self.create_user(name).await;
        self.enroll_user(schema, user, seed).await
    }

    /// [`TestCtx::enrolled`] for an existing `user`, from a new password session.
    pub async fn enroll_user(&self, schema: &AppSchema, user: User, seed: u8) -> Enrolled {
        let caller = self.caller(&user).await;
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
        Enrolled {
            user,
            caller,
            identity,
            state,
            device_id,
            device,
        }
    }
}

/// The input of `signupWithKey` for `username` with the identity `id`: its inception as the key
/// log, a new device from `device_seed`, a fresh login proof and the registration of
/// [`TEST_PASSWORD`] (made only when `extra` brings no `passwordRegistration`); `extra` adds
/// input fields (a token) or replaces them.
pub async fn signup_input(
    schema: &AppSchema,
    username: &str,
    id: &TestIdentity,
    device_seed: u8,
    extra: Value,
) -> Value {
    let inception = id.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = Uuid::now_v7();
    let device = signing_key(device_seed);
    let cert = id.device_cert(&state, device_id, &device, "Laptop");
    let proof = signed_login(schema, &device, &state, device_id).await;
    let password_registration = match extra.get("passwordRegistration") {
        Some(given) => given.clone(),
        None => json!(registration(schema, &state.principal_id, TEST_PASSWORD).await),
    };
    let mut input = json!({
        "username": username,
        "keyLog": [statement(&inception)],
        "device": statement(&cert),
        "proof": statement(&proof),
        "passwordRegistration": password_registration,
    });
    if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        input.extend(extra.clone());
    }
    input
}

/// Sign in on `device` (id `device_id`, under the key log head `state`) with a fresh proof
/// and, when `password` is given, a password login for it. Returns the answer, or the refusal
/// of the password login's start, and the password login, whose session key opens the server
/// share.
pub async fn key_login(
    schema: &AppSchema,
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
    password: Option<&[u8]>,
) -> (async_graphql::Response, Option<PasswordLogin>) {
    let password_login = match password {
        Some(password) => match login(schema, &state.principal_id, password).await {
            Ok(login) => login,
            Err(refused) => return (refused, None),
        },
        None => None,
    };
    let proof = signed_login(schema, device, state, device_id).await;
    let variables = match &password_login {
        Some(login) => login.variables(statement(&proof)),
        None if password.is_some() => json!({
            "proof": statement(&proof),
            "loginId": Uuid::new_v4().to_string(),
            "credentialFinalization": b64(&[0; 64]),
        }),
        None => json!({ "proof": statement(&proof) }),
    };
    (
        schema
            .execute(signed_out(LOGIN_WITH_PASSWORD, variables))
            .await,
        password_login,
    )
}
