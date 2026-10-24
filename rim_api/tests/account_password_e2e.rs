//! Resolver-level e2e tests of the account password over a real database: signup with a
//! password and sign-in with it, wrong tries and the lock, change, reset with the recovery key,
//! and the session of an account that has no password yet.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use async_graphql::Request;
use serde_json::{Value, json};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_api::auth_context::{AuthActorCtx, RemoteIp};
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
use rim_auth::opaque::client::{Login, Registration};
use rim_auth::opaque::{LABEL_SERVER_SHARE, transport_open};
use rim_core::auth::repository::SessionRepository;
use rim_identity::testing::{SigningKey, TestIdentity, signing_key};
use rim_identity::{KeyState, StatementVersion};
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_services::auth::dto::RegistrationMode;
use rim_services::provider::AppServiceProvider;
use sea_orm::ConnectionTrait;

use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, key_login, signed_out, signup_input, statement};
use support::password::{
    LOGIN_FINISH, LOGIN_START, LOGIN_WITH_PASSWORD, SET_PASSWORD, SET_PASSWORD_START,
    TEST_PASSWORD, b64, login as password_login, unb64,
};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

const SIGNUP_RETURNING_SHARE: &str = r#"
mutation Signup($input: SignupWithKeyInput!) { signupWithKey(input: $input) { userId serverShare } }"#;

const CHANGE_START: &str = r#"
mutation Start($credentialRequest: String!, $registrationRequest: String!) {
  passwordChangeStart(credentialRequest: $credentialRequest, registrationRequest: $registrationRequest) {
    loginId credentialResponse registrationResponse
  }
}"#;

const CHANGE_FINISH: &str = r#"
mutation Finish($loginId: ID!, $credentialFinalization: String!, $registrationUpload: String!, $rewrap: String!) {
  passwordChangeFinish(loginId: $loginId, credentialFinalization: $credentialFinalization,
                       registrationUpload: $registrationUpload, rewrap: $rewrap) { changedAt }
}"#;

const RESET_CHALLENGE: &str = r#"
mutation Challenge($principalId: String!) {
  passwordResetChallenge(principalId: $principalId) { challengeId nonce }
}"#;

const RESET_START: &str = r#"
mutation Start($statement: SignedStatementInput!, $registrationRequest: String!) {
  passwordResetStart(statement: $statement, registrationRequest: $registrationRequest) {
    resetId registrationResponse
  }
}"#;

const RESET_FINISH: &str = r#"
mutation Finish($resetId: ID!, $registrationUpload: String!) {
  passwordResetFinish(resetId: $resetId, registrationUpload: $registrationUpload)
}"#;

const MY_KEY_IDENTITY: &str = "query { myKeyIdentity { principalId } }";

/// An account signed up with a key and [`TEST_PASSWORD`].
struct SignedUp {
    user_id: String,
    identity: TestIdentity,
    state: KeyState,
    device: SigningKey,
    device_id: Uuid,
    server_share: Vec<u8>,
}

/// Sign up `username` with the identity from `seed` on a device whose id and key come from
/// `seed` too.
async fn sign_up(schema: &AppSchema, username: &str, seed: u8) -> SignedUp {
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let state = KeyState::from_inception(&identity.inception()).expect("inception");
    let device_seed = seed.wrapping_add(150);
    let input = signup_input(schema, username, &identity, device_seed, json!({})).await;
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
    let data = exec_ok(
        schema,
        signed_out(SIGNUP_RETURNING_SHARE, json!({ "input": input })),
    )
    .await;
    SignedUp {
        user_id: data["signupWithKey"]["userId"]
            .as_str()
            .expect("userId")
            .to_string(),
        identity,
        state,
        device: signing_key(device_seed),
        device_id,
        server_share: unb64(
            data["signupWithKey"]["serverShare"]
                .as_str()
                .expect("serverShare"),
        ),
    }
}

impl SignedUp {
    async fn login(&self, schema: &AppSchema, password: Option<&[u8]>) -> async_graphql::Response {
        key_login(schema, &self.device, &self.state, self.device_id, password)
            .await
            .0
    }

    fn uid(&self) -> rim_core::user::model::UserId {
        self.user_id.parse::<Uuid>().expect("uuid").into()
    }
}

/// The first error code of `response`.
fn code(response: &async_graphql::Response) -> String {
    match response
        .errors
        .first()
        .unwrap_or_else(|| panic!("expected an error, got {:?}", response.data))
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
    {
        Some(async_graphql::Value::String(code)) => code.clone(),
        other => panic!("no code: {other:?}"),
    }
}

fn data(response: async_graphql::Response) -> Value {
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().expect("json")
}

#[tokio::test]
async fn signup_with_a_password_then_sign_in_returns_the_same_share() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 1).await;
    assert_eq!(ada.server_share.len(), 32);

    let (response, login) = key_login(
        &schema,
        &ada.device,
        &ada.state,
        ada.device_id,
        Some(TEST_PASSWORD),
    )
    .await;
    let body = data(response);
    let answer = &body["loginWithKey"];
    assert_eq!(answer["userId"], json!(ada.user_id));
    assert_eq!(answer["mustSetPassword"], json!(false));
    let login = login.expect("password login");
    assert_eq!(login.rewraps, json!([]));
    let share = transport_open(&login.session_key, LABEL_SERVER_SHARE, &login.server_share)
        .expect("the share opens under the login's transport key");
    assert_eq!(share.as_slice(), ada.server_share.as_slice());
}

#[tokio::test]
async fn key_login_alone_or_with_a_wrong_password_is_refused_for_an_account_with_a_password() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 2).await;
    assert_eq!(code(&ada.login(&schema, None).await), "UNAUTHENTICATED");
    assert_eq!(
        code(&ada.login(&schema, Some(b"wrong password")).await),
        "UNAUTHENTICATED"
    );
    // A wrong finalization gets no ticket.
    let (_, request) = Login::start(TEST_PASSWORD);
    let started = exec_ok(
        &schema,
        signed_out(
            LOGIN_START,
            json!({ "principalId": ada.state.principal_id, "request": b64(&request) }),
        ),
    )
    .await;
    assert_eq!(
        exec_err_code(
            &schema,
            signed_out(
                LOGIN_FINISH,
                json!({
                    "loginId": started["passwordLoginStart"]["loginId"],
                    "credentialFinalization": b64(&[0; 64]),
                }),
            ),
        )
        .await,
        "UNAUTHENTICATED"
    );

    // A ticket signs in once, and only its own account.
    let bob = sign_up(&schema, "bob", 12).await;
    let login = password_login(&schema, &ada.state.principal_id, TEST_PASSWORD)
        .await
        .expect("start")
        .expect("right password");
    assert_eq!(
        code(&with_ticket(&schema, &bob, &login).await),
        "UNAUTHENTICATED"
    );
    assert!(with_ticket(&schema, &ada, &login).await.errors.is_empty());
    assert_eq!(
        code(&with_ticket(&schema, &ada, &login).await),
        "UNAUTHENTICATED"
    );
}

/// `loginWithKey` for `account` with a fresh proof and `login`'s ticket.
async fn with_ticket(
    schema: &AppSchema,
    account: &SignedUp,
    login: &support::password::PasswordLogin,
) -> async_graphql::Response {
    let proof =
        support::identity::signed_login(schema, &account.device, &account.state, account.device_id)
            .await;
    schema
        .execute(signed_out(
            LOGIN_WITH_PASSWORD,
            login.variables(statement(&proof)),
        ))
        .await
}

#[tokio::test]
async fn an_expired_ticket_is_refused() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 13).await;
    let login = password_login(&schema, &ada.state.principal_id, TEST_PASSWORD)
        .await
        .expect("start")
        .expect("right password");
    ctx.conn
        .execute_unprepared("UPDATE password_ticket SET expires_at = now() - interval '1 second'")
        .await
        .expect("expire the ticket");
    assert_eq!(
        code(&with_ticket(&schema, &ada, &login).await),
        "UNAUTHENTICATED"
    );
}

#[tokio::test]
async fn ten_tries_without_a_success_lock_the_password_until_a_reset() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 3).await;
    // Nine tries are behind it and the wait after the ninth has run out.
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE account_password SET failed_attempts = 9, next_attempt_at = now() \
             - interval '1 second' WHERE user_id = '{}'",
            ada.user_id
        ))
        .await
        .expect("set the count");
    // The tenth gets an answer and locks the password.
    assert_eq!(
        code(&ada.login(&schema, Some(b"wrong")).await),
        "UNAUTHENTICATED"
    );
    let response = ada.login(&schema, Some(TEST_PASSWORD)).await;
    assert_eq!(code(&response), "PASSWORD_LOCKED");

    // A reset with the recovery key opens it again.
    reset(&schema, &ada, b"new password").await;
    assert!(
        ada.login(&schema, Some(b"new password"))
            .await
            .errors
            .is_empty()
    );
}

#[tokio::test]
async fn a_try_during_the_wait_is_rate_limited_with_the_seconds_left() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 4).await;
    for _ in 0..4 {
        assert_eq!(
            code(&ada.login(&schema, Some(b"wrong")).await),
            "UNAUTHENTICATED"
        );
    }
    let response = ada.login(&schema, Some(TEST_PASSWORD)).await;
    assert_eq!(code(&response), "RATE_LIMITED");
    let retry = response.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("retryAfterSeconds"))
        .cloned();
    assert!(
        matches!(retry, Some(async_graphql::Value::Number(ref n)) if n.as_u64().is_some_and(|s| (1..=60).contains(&s))),
        "{retry:?}"
    );
}

/// A session of `account` made directly, as a signed-in device holds one.
async fn caller(ctx: &TestCtx, account: &SignedUp) -> Caller {
    let user = rim_core::user::repository::UserRepository::get_by_id(
        &rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository::new(ctx.db.clone()),
        account.uid(),
    )
    .await
    .expect("get user")
    .expect("user");
    ctx.caller(&user).await
}

/// Change `account`'s password from `old` to `new`, from `caller`'s session.
async fn change(
    schema: &AppSchema,
    caller: &Caller,
    old: &[u8],
    new: &[u8],
) -> async_graphql::Response {
    let (login, credential_request) = Login::start(old);
    let (registration, registration_request) = Registration::start(new);
    let started = exec_ok(
        schema,
        caller.request(
            CHANGE_START,
            json!({
                "credentialRequest": b64(&credential_request),
                "registrationRequest": b64(&registration_request),
            }),
        ),
    )
    .await;
    let started = &started["passwordChangeStart"];
    let finalization = login
        .finish(&unb64(
            started["credentialResponse"]
                .as_str()
                .expect("credentialResponse"),
        ))
        .map_or_else(|| b64(&[0; 64]), |done| b64(&done.finalization));
    let (upload, _) = registration
        .finish(&unb64(
            started["registrationResponse"]
                .as_str()
                .expect("registrationResponse"),
        ))
        .expect("upload");
    schema
        .execute(caller.request(
            CHANGE_FINISH,
            json!({
                "loginId": started["loginId"],
                "credentialFinalization": finalization,
                "registrationUpload": b64(&upload),
                "rewrap": b64(&[7; 60]),
            }),
        ))
        .await
}

#[tokio::test]
async fn a_change_refuses_the_old_password_afterwards_and_hands_out_the_rewrap() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 5).await;
    let caller = caller(&ctx, &ada).await;

    assert_eq!(
        code(&change(&schema, &caller, b"not the password", b"new").await),
        "UNAUTHENTICATED"
    );
    let changed = data(change(&schema, &caller, TEST_PASSWORD, b"new password").await);
    assert!(changed["passwordChangeFinish"]["changedAt"].is_string());

    assert_eq!(
        code(&ada.login(&schema, Some(TEST_PASSWORD)).await),
        "UNAUTHENTICATED"
    );
    let (response, login) = key_login(
        &schema,
        &ada.device,
        &ada.state,
        ada.device_id,
        Some(b"new password"),
    )
    .await;
    data(response);
    let login = login.expect("login");
    assert_eq!(login.rewraps, json!([b64(&[7; 60])]));
    let share =
        transport_open(&login.session_key, LABEL_SERVER_SHARE, &login.server_share).expect("opens");
    assert_eq!(
        share.as_slice(),
        ada.server_share.as_slice(),
        "the share stays"
    );
}

/// Reset `account`'s password to `new` with a statement signed by its recovery key.
async fn reset(schema: &AppSchema, account: &SignedUp, new: &[u8]) {
    let challenge = exec_ok(
        schema,
        signed_out(
            RESET_CHALLENGE,
            json!({ "principalId": account.state.principal_id }),
        ),
    )
    .await;
    let challenge = &challenge["passwordResetChallenge"];
    let statement_signed = account.identity.password_reset(
        &account.state,
        challenge["challengeId"]
            .as_str()
            .expect("challengeId")
            .parse()
            .expect("uuid"),
        challenge["nonce"].as_str().expect("nonce"),
        KEY_LOGIN_AUDIENCE,
    );
    let (registration, request) = Registration::start(new);
    let started = exec_ok(
        schema,
        signed_out(
            RESET_START,
            json!({
                "statement": statement(&statement_signed),
                "registrationRequest": b64(&request),
            }),
        ),
    )
    .await;
    let started = &started["passwordResetStart"];
    let (upload, _) = registration
        .finish(&unb64(
            started["registrationResponse"]
                .as_str()
                .expect("registrationResponse"),
        ))
        .expect("upload");
    let done = exec_ok(
        schema,
        signed_out(
            RESET_FINISH,
            json!({ "resetId": started["resetId"], "registrationUpload": b64(&upload) }),
        ),
    )
    .await;
    assert_eq!(done["passwordResetFinish"], json!(true));
}

#[tokio::test]
async fn a_reset_signed_by_the_recovery_key_replaces_the_password_and_ends_every_session() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 6).await;
    let caller = caller(&ctx, &ada).await;
    assert!(
        data(change(&schema, &caller, TEST_PASSWORD, b"second").await)["passwordChangeFinish"]
            .is_object()
    );

    reset(&schema, &ada, b"after reset").await;
    let session = SeaOrmSessionRepository::new(ctx.db.clone())
        .get_by_id(caller.session_id)
        .await
        .expect("get session")
        .expect("session");
    assert!(session.revoked_at.is_some(), "the reset ended the session");
    assert_eq!(
        code(&ada.login(&schema, Some(b"second")).await),
        "UNAUTHENTICATED"
    );
    let (response, login) = key_login(
        &schema,
        &ada.device,
        &ada.state,
        ada.device_id,
        Some(b"after reset"),
    )
    .await;
    data(response);
    let login = login.expect("login");
    assert_eq!(login.rewraps, json!([]), "rewraps go");
    let share =
        transport_open(&login.session_key, LABEL_SERVER_SHARE, &login.server_share).expect("opens");
    assert_eq!(share.as_slice(), ada.server_share.as_slice());

    // A statement signed by the identity key does not reset.
    let challenge = exec_ok(
        &schema,
        signed_out(
            RESET_CHALLENGE,
            json!({ "principalId": ada.state.principal_id }),
        ),
    )
    .await;
    let challenge = &challenge["passwordResetChallenge"];
    let forged = rim_identity::sign(
        rim_identity::Purpose::PasswordReset,
        StatementVersion::V2,
        &ada.identity.identity,
        rim_identity::testing::password_reset_payload(
            &ada.state,
            challenge["challengeId"]
                .as_str()
                .expect("id")
                .parse()
                .expect("uuid"),
            challenge["nonce"].as_str().expect("nonce"),
            KEY_LOGIN_AUDIENCE,
        ),
    );
    let (_, request) = Registration::start(b"x");
    assert_eq!(
        exec_err_code(
            &schema,
            signed_out(
                RESET_START,
                json!({ "statement": statement(&forged), "registrationRequest": b64(&request) }),
            ),
        )
        .await,
        "UNAUTHENTICATED"
    );
}

/// The request a server makes for `session_token`: the authenticated session in the context,
/// with the limit of a session that must set a password.
async fn as_session(
    provider: &Arc<dyn AppServiceProvider>,
    session_token: &str,
    query: &str,
    variables: Value,
) -> Request {
    let auth = provider
        .request_authenticator()
        .authenticate_session_token(Some(session_token))
        .await
        .expect("authenticate");
    anonymous_request(query, variables).data(
        AuthActorCtx::new(auth.user_id, auth.session_id)
            .with_device_key_id(auth.device_key_id)
            .with_must_set_password(auth.must_set_password),
    )
}

#[tokio::test]
async fn an_account_without_a_password_signs_in_with_its_key_and_may_only_set_one() {
    let ctx = TestCtx::new().await;
    let (provider, observers) = support::build_test_provider(&ctx.conn);
    let schema = support::schema_for(
        Arc::clone(&provider),
        observers,
        RegistrationMode::InviteOnly,
    );
    let lin = ctx.enrolled(&schema, "lin", 7).await;

    let proof =
        support::identity::signed_login(&schema, &lin.device, &lin.state, lin.device_id).await;
    let intent = Arc::new(AuthIntentCtx::default());
    let body = exec_ok(
        &schema,
        anonymous_request(LOGIN_WITH_PASSWORD, json!({ "proof": statement(&proof) }))
            .data(RemoteIp::new("198.51.100.30"))
            .data(Arc::clone(&intent)),
    )
    .await;
    assert_eq!(body["loginWithKey"]["mustSetPassword"], json!(true));
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("loginWithKey must set a session");
    };

    // Everything but setting a password, signing out and `me` is refused.
    let refused = schema
        .execute(as_session(&provider, &session_token, MY_KEY_IDENTITY, json!({})).await)
        .await;
    assert_eq!(code(&refused), "FORBIDDEN");
    assert_eq!(refused.errors[0].message, "set a password first");
    let me = schema
        .execute(as_session(&provider, &session_token, "query { me { id } }", json!({})).await)
        .await;
    assert!(me.errors.is_empty(), "{:?}", me.errors);

    let (registration, request) = Registration::start(b"first password");
    let started = data(
        schema
            .execute(
                as_session(
                    &provider,
                    &session_token,
                    SET_PASSWORD_START,
                    json!({ "request": b64(&request) }),
                )
                .await,
            )
            .await,
    );
    let (upload, _) = registration
        .finish(&unb64(
            started["setPasswordStart"].as_str().expect("response"),
        ))
        .expect("upload");
    let set = data(
        schema
            .execute(
                as_session(
                    &provider,
                    &session_token,
                    SET_PASSWORD,
                    json!({ "upload": b64(&upload) }),
                )
                .await,
            )
            .await,
    );
    assert_eq!(
        unb64(set["setPassword"]["serverShare"].as_str().expect("share")).len(),
        32
    );
    let allowed = schema
        .execute(as_session(&provider, &session_token, MY_KEY_IDENTITY, json!({})).await)
        .await;
    assert!(allowed.errors.is_empty(), "{:?}", allowed.errors);

    // From now on the key alone no longer signs in.
    let (response, _) = key_login(&schema, &lin.device, &lin.state, lin.device_id, None).await;
    assert_eq!(code(&response), "UNAUTHENTICATED");
    let (response, _) = key_login(
        &schema,
        &lin.device,
        &lin.state,
        lin.device_id,
        Some(b"first password"),
    )
    .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[tokio::test]
async fn someone_from_another_server_signs_in_directly_without_a_password() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let guest = ctx.enrolled(&schema, "guest", 8).await;
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE \"user\" SET kind = 'remote' WHERE id = '{}'",
            guest.user.id
        ))
        .await
        .expect("make remote");
    let (response, _) =
        key_login(&schema, &guest.device, &guest.state, guest.device_id, None).await;
    let body = data(response);
    assert_eq!(body["loginWithKey"]["mustSetPassword"], json!(false));
}

#[tokio::test]
async fn registration_is_refused_for_a_principal_an_account_here_holds() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = sign_up(&schema, "ada", 9).await;
    let (_, request) = Registration::start(TEST_PASSWORD);
    assert_eq!(
        exec_err_code(
            &schema,
            signed_out(
                support::password::REGISTRATION_START,
                json!({ "principalId": ada.state.principal_id, "request": b64(&request) }),
            ),
        )
        .await,
        "CONFLICT"
    );
    // A signup without the password registration does not parse.
    let id = TestIdentity::with_version(10, StatementVersion::V2);
    let mut input = signup_input(&schema, "bob", &id, 160, json!({})).await;
    if let Some(input) = input.as_object_mut() {
        input.remove("passwordRegistration");
    }
    let response = schema
        .execute(signed_out(SIGNUP_WITH_KEY, json!({ "input": input })))
        .await;
    assert!(!response.errors.is_empty());
}
