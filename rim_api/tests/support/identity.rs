/// An account signed up with a key identity and a password, and the statement helpers the key
/// identity, device link and encryption suites share.
use std::sync::Arc;

use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_api::auth_context::{AuthActorCtx, RemoteIp};
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
use rim_services::provider::AppServiceProvider;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use super::graphql::{Caller, anonymous_request, exec_ok};
use super::password::{
    LOGIN_WITH_PASSWORD, PasswordLogin, TEST_PASSWORD, login, registration, unb64,
};
use super::{KEY_LOGIN_AUDIENCE, TestCtx};

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
    /// The account's user row.
    pub async fn user(&self, ctx: &TestCtx) -> User {
        SeaOrmUserRepository::new(ctx.db.clone())
            .get_by_id(self.user_id)
            .await
            .expect("get user")
            .expect("the signed-up user")
    }

    /// Sign in on the account's device through `schema` with [`TEST_PASSWORD`]; returns the
    /// new session's token, the value of its cookie.
    pub async fn sign_in(&self, schema: &AppSchema) -> String {
        let (request, _) = key_login_request(
            schema,
            &self.device,
            &self.state,
            self.device_id,
            Some(TEST_PASSWORD),
        )
        .await
        .expect("password login");
        let intent = Arc::new(AuthIntentCtx::default());
        exec_ok(schema, request.data(Arc::clone(&intent))).await;
        session_token(&intent)
    }

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
    signed_up_with(ctx, username, seed, StatementVersion::V2).await
}

/// [`signed_up`] with an identity whose statements are written at `version`.
pub async fn signed_up_with(
    ctx: &TestCtx,
    username: &str,
    seed: u8,
    version: StatementVersion,
) -> SignedUp {
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    signed_up_through(ctx, &schema, username, seed, version, json!({})).await
}

/// Sign up `username` through `schema` as [`signed_up`] does, with an identity whose statements
/// are written at `version` and `extra` input fields (a token).
pub async fn signed_up_through(
    ctx: &TestCtx,
    schema: &AppSchema,
    username: &str,
    seed: u8,
    version: StatementVersion,
    extra: Value,
) -> SignedUp {
    let identity = TestIdentity::with_version(seed, version);
    let state = KeyState::from_inception(&identity.inception()).expect("inception");
    let device_seed = seed.wrapping_add(50);
    let input = signup_input(schema, username, &identity, device_seed, extra).await;
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
        schema,
        signed_out(SIGNUP_RETURNING_SHARE, json!({ "input": input })).data(Arc::clone(&intent)),
    )
    .await;
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
        caller: session_of(ctx, user_id, &intent).await,
    }
}

/// An account with a version 2 key identity, signed in on its first device.
pub struct Enrolled {
    pub user: User,
    /// The signup's session, bound to `device_id`.
    pub caller: Caller,
    pub identity: TestIdentity,
    pub state: KeyState,
    pub device_id: Uuid,
    pub device: SigningKey,
}

const PUBLISH_CARD: &str = r#"
mutation Publish($card: SignedStatementInput!) { publishProfileCard(card: $card) { version } }"#;

impl Enrolled {
    /// Publish a first profile card naming the account's device, through `schema`, as a client
    /// does before it starts a conversation with someone here.
    pub async fn publish_card(&self, schema: &AppSchema) {
        let card = self.identity.profile_card(
            &self.state,
            1,
            &self.user.username,
            &[],
            &[(self.device_id, &self.device)],
        );
        exec_ok(
            schema,
            self.caller
                .request(PUBLISH_CARD, json!({ "card": statement(&card) })),
        )
        .await;
    }
}

impl TestCtx {
    /// A user `name` signed up with the identity from `seed`, the device key from `seed + 50`
    /// and [`TEST_PASSWORD`] (see [`signed_up`]), with the email `<name>@test.example.com` that
    /// [`TestCtx::create_user`] gives.
    pub async fn enrolled(&self, name: &str, seed: u8) -> Enrolled {
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
    match key_login_request(schema, device, state, device_id, password).await {
        Ok((request, password_login)) => (schema.execute(request).await, password_login),
        Err(refused) => (refused, None),
    }
}

/// The `loginWithKey` request [`key_login`] sends, after its password login, for a caller that
/// adds data to it; the refusal of a password login step instead.
pub async fn key_login_request(
    schema: &AppSchema,
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
    password: Option<&[u8]>,
) -> Result<(Request, Option<PasswordLogin>), async_graphql::Response> {
    let password_login = match password {
        Some(password) => login(schema, &state.principal_id, password).await?,
        None => None,
    };
    let proof = signed_login(schema, device, state, device_id).await;
    let variables = match &password_login {
        Some(login) => login.variables(statement(&proof)),
        None => json!({ "proof": statement(&proof) }),
    };
    Ok((signed_out(LOGIN_WITH_PASSWORD, variables), password_login))
}

/// The token of the session `intent` set.
///
/// # Panics
/// When the mutation set no session.
fn session_token(intent: &AuthIntentCtx) -> String {
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("the mutation must set a session");
    };
    session_token
}

/// The session `intent` set for `user_id`, as a [`Caller`].
async fn session_of(ctx: &TestCtx, user_id: UserId, intent: &AuthIntentCtx) -> Caller {
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_token_hash(&hash_token(&session_token(intent)))
        .await
        .expect("get session")
        .expect("the new session");
    Caller {
        user_id,
        session_id: session.id,
    }
}

/// A request carrying `session_token` as the session cookie, resolved the way the HTTP layer
/// resolves it: a revoked or unknown session yields no actor.
pub async fn with_session(
    provider: &Arc<dyn AppServiceProvider>,
    session_token: &str,
    query: &str,
) -> Request {
    let auth = provider
        .request_authenticator()
        .authenticate_session_token(
            Some(session_token),
            rim_services::auth::dto::SessionSecret::NotRequired,
        )
        .await
        .expect("authenticate");
    Request::new(query).data(AuthActorCtx::new(auth.user_id, auth.session_id))
}
