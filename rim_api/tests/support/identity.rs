/// An account signed up with a key identity and a password, and the statement helpers the key
/// identity, device link and encryption suites share.
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
use rim_core::user::model::{User, UserId};
use rim_core::user::repository::UserRepository;
use rim_identity::testing::{SigningKey, TestIdentity, device_login, signing_key};
use rim_identity::{KeyState, SignedStatement, StatementVersion};
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use super::graphql::{Caller, anonymous_request, exec_ok};
use super::password::{
    LOGIN_WITH_PASSWORD, PasswordLogin, TEST_PASSWORD, login, registration, unb64,
};
use super::{KEY_LOGIN_AUDIENCE, TestCtx};

const ENROLL: &str = r#"
mutation Enroll($input: EnrollIdentityKeyInput!) { enrollIdentityKey(input: $input) { principalId } }"#;
const CHALLENGE: &str = "mutation { keyLoginChallenge { challengeId nonce } }";

pub const SIGNUP_WITH_KEY: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId serverId } }"#;

const SIGNUP_RETURNING_SHARE: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId serverShare } }"#;

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

/// An account signed up with a key and [`TEST_PASSWORD`], signed in on its first device.
pub struct SignedUp {
    pub user_id: UserId,
    pub principal_id: String,
    pub identity: TestIdentity,
    pub state: KeyState,
    pub device_id: Uuid,
    pub device: SigningKey,
    /// The server share the signup returned, in the clear.
    pub server_share: Vec<u8>,
    /// The session the signup opened, bound to `device_id`.
    pub caller: Caller,
}

impl SignedUp {
    /// Sign in on the account's device through `schema`; see [`key_login`].
    pub async fn login(
        &self,
        schema: &AppSchema,
        password: Option<&[u8]>,
    ) -> async_graphql::Response {
        key_login(schema, &self.device, &self.state, self.device_id, password)
            .await
            .0
    }
}

/// Sign up `username` through `signupWithKey`, with the version 2 identity from `seed`, the
/// device key from `seed + 50` and [`TEST_PASSWORD`]. The signup runs on a schema of its own
/// over the test database, with open registration, so it neither needs a token nor counts
/// against the limits of the schema under test; every schema of a test binary shares the
/// password key, so the password signs in through any of them.
pub async fn signed_up(ctx: &TestCtx, username: &str, seed: u8) -> SignedUp {
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let state = KeyState::from_inception(&identity.inception()).expect("inception");
    let device_seed = seed.wrapping_add(50);
    let input = signup_input(&schema, username, &identity, device_seed, json!({})).await;
    let device_id: Uuid = {
        let payload: Value = serde_json::from_str(
            input["device"]["payload"]
                .as_str()
                .expect("certificate payload"),
        )
        .expect("certificate json");
        payload["deviceId"]
            .as_str()
            .expect("deviceId")
            .parse()
            .expect("uuid")
    };
    let intent = Arc::new(AuthIntentCtx::default());
    let data = exec_ok(
        &schema,
        signed_out(SIGNUP_RETURNING_SHARE, json!({ "input": input })).data(Arc::clone(&intent)),
    )
    .await;
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("signupWithKey must set a session");
    };
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_token_hash(&hash_token(&session_token))
        .await
        .expect("get session")
        .expect("the signup's session");
    let user_id: UserId = data["signupWithKey"]["userId"]
        .as_str()
        .expect("userId")
        .parse::<Uuid>()
        .expect("uuid")
        .into();
    SignedUp {
        user_id,
        principal_id: state.principal_id.clone(),
        identity,
        state,
        device_id,
        device: signing_key(device_seed),
        server_share: unb64(
            data["signupWithKey"]["serverShare"]
                .as_str()
                .expect("serverShare"),
        ),
        caller: Caller {
            user_id,
            session_id: session.id,
        },
    }
}

/// An account with a version 2 key identity, signed in on its first device.
pub struct Enrolled {
    pub user: User,
    /// The account's session, bound to `device_id`: the signup's, or the one that enrolled the
    /// identity.
    pub caller: Caller,
    pub identity: TestIdentity,
    pub state: KeyState,
    pub device_id: Uuid,
    pub device: SigningKey,
}

impl TestCtx {
    /// A user `name` signed up with the identity from `seed`, the device key from `seed + 50`
    /// and [`TEST_PASSWORD`] (see [`signed_up`]), with the email `<name>@test.example.com` that
    /// [`TestCtx::create_user`] gives. `schema` is the schema under test, which the signup does
    /// not go through.
    pub async fn enrolled(&self, _schema: &AppSchema, name: &str, seed: u8) -> Enrolled {
        let account = signed_up(self, name, seed).await;
        self.conn
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE "user" SET email = $1 WHERE id = $2"#,
                [
                    format!("{}@test.example.com", name.to_lowercase()).into(),
                    Uuid::from(account.user_id).into(),
                ],
            ))
            .await
            .expect("set the email");
        let user = SeaOrmUserRepository::new(self.db.clone())
            .get_by_id(account.user_id)
            .await
            .expect("get user")
            .expect("the signed-up user");
        Enrolled {
            user,
            caller: account.caller,
            identity: account.identity,
            state: account.state,
            device_id: account.device_id,
            device: account.device,
        }
    }

    /// An existing `user` who enrolled the identity from `seed` with the device key from
    /// `seed + 50`, named "Desktop", from a new password session.
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
        "keepSignedIn": true,
        "keepKeyHeld": true,
    });
    if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        input.extend(extra.clone());
    }
    input
}

/// Sign in on `device` (id `device_id`, under the key log head `state`) with a fresh proof
/// and, when `password` is given and right, the ticket of a password login. Returns the
/// answer, or the refusal of a password login step, and the password login, whose session key
/// opens the server share it answered.
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
        None => json!({ "proof": statement(&proof) }),
    };
    (
        schema
            .execute(signed_out(LOGIN_WITH_PASSWORD, variables))
            .await,
        password_login,
    )
}
