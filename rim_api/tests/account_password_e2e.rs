//! Resolver-level e2e tests of the account password over a real database: signup with a
//! password and sign-in with it, wrong tries and the lock, change, and reset with the recovery
//! key.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
use rim_auth::opaque::client::{Login, Registration};
use rim_auth::opaque::{LABEL_SERVER_SHARE, transport_open};
use rim_core::auth::repository::SessionRepository;
use rim_identity::StatementVersion;
use rim_identity::testing::TestIdentity;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_services::auth::dto::RegistrationMode;
use sea_orm::ConnectionTrait;

use support::graphql::{Caller, exec_err_code, exec_ok};
use support::identity::{
    SIGNUP_WITH_KEY, SignedUp, key_login, signed_out, signed_up, signup_input, statement,
};
use support::password::{
    LOGIN_FINISH, LOGIN_START, LOGIN_WITH_PASSWORD, RESET_CHALLENGE, RESET_START, TEST_PASSWORD,
    b64, login as password_login, reset_password, unb64,
};
use support::{KEY_LOGIN_AUDIENCE, TestCtx};

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
    let ada = signed_up(&ctx, "ada", 1).await;
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
    assert_eq!(answer["userId"], json!(ada.user_id.to_string()));
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
    let ada = signed_up(&ctx, "ada", 2).await;
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
    let bob = signed_up(&ctx, "bob", 12).await;
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

/// A password sign-in chooses what its session keeps: unticked, a session secret and
/// browser-session cookies; ticked on the web, a keep key; ticked on the desktop, neither.
#[tokio::test]
async fn a_key_login_hands_out_the_secret_or_keep_key_it_chose() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 14).await;
    for (keep_signed_in, keep_key_held) in [(false, false), (true, false), (true, true)] {
        let login = password_login(&schema, &ada.state.principal_id, TEST_PASSWORD)
            .await
            .expect("start")
            .expect("right password");
        let proof =
            support::identity::signed_login(&schema, &ada.device, &ada.state, ada.device_id).await;
        let mut variables = login.variables(statement(&proof));
        variables["keepSignedIn"] = json!(keep_signed_in);
        variables["keepKeyHeld"] = json!(keep_key_held);
        let intent = Arc::new(AuthIntentCtx::default());
        let body = exec_ok(
            &schema,
            signed_out(LOGIN_WITH_PASSWORD, variables).data(Arc::clone(&intent)),
        )
        .await;
        let answer = &body["loginWithKey"];
        let Some(AuthIntent::SetSession { kept, .. }) = intent.take() else {
            panic!("loginWithKey must set a session");
        };
        assert_eq!(kept, keep_signed_in);
        let secret = answer["sessionSecret"].as_str();
        let keep_key = answer["keepKey"].as_str();
        match (keep_signed_in, keep_key_held) {
            (false, _) => {
                assert_eq!(secret.map(|s| unb64(s).len()), Some(32));
                assert!(keep_key.is_none());
            }
            (true, false) => {
                assert!(secret.is_none());
                assert_eq!(keep_key.map(|k| unb64(k).len()), Some(32));
            }
            (true, true) => assert!(secret.is_none() && keep_key.is_none()),
        }
    }
}

#[tokio::test]
async fn an_expired_ticket_is_refused() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 13).await;
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

fn retry_after(response: &async_graphql::Response) -> Option<u64> {
    match response.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("retryAfterSeconds"))
    {
        Some(async_graphql::Value::Number(n)) => n.as_u64(),
        _ => None,
    }
}

#[tokio::test]
async fn ten_starts_an_hour_then_a_wait_until_the_window_ends_and_never_a_lock() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 3).await;
    // Nine starts in a window that ends in half an hour; the tenth still gets an answer.
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE account_password SET failed_attempts = 9, \
             next_attempt_at = now() + interval '30 minutes' WHERE user_id = '{}'",
            ada.user_id
        ))
        .await
        .expect("fill the window");
    assert_eq!(
        code(&ada.login(&schema, Some(b"wrong")).await),
        "UNAUTHENTICATED"
    );
    let response = ada.login(&schema, Some(TEST_PASSWORD)).await;
    assert_eq!(code(&response), "RATE_LIMITED");
    let retry = retry_after(&response);
    assert!(
        retry.is_some_and(|s| (1700..=1800).contains(&s)),
        "{retry:?}"
    );

    // Once the window ends the right password signs in, with no reset.
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE account_password SET next_attempt_at = now() - interval '1 second' \
             WHERE user_id = '{}'",
            ada.user_id
        ))
        .await
        .expect("end the window");
    assert!(
        ada.login(&schema, Some(TEST_PASSWORD))
            .await
            .errors
            .is_empty()
    );
}

#[tokio::test]
async fn a_password_locked_or_held_back_for_hours_before_takes_starts_again() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 4).await;
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE account_password SET failed_attempts = 10, locked_at = now(), \
             next_attempt_at = now() + interval '12 hours' WHERE user_id = '{}'",
            ada.user_id
        ))
        .await
        .expect("lock the password");
    assert!(
        ada.login(&schema, Some(TEST_PASSWORD))
            .await
            .errors
            .is_empty()
    );
}

/// A session of `account` made directly, as a signed-in device holds one.
async fn caller(ctx: &TestCtx, account: &SignedUp) -> Caller {
    let user = rim_core::user::repository::UserRepository::get_by_id(
        &rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository::new(ctx.db.clone()),
        account.user_id,
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
    let ada = signed_up(&ctx, "ada", 5).await;
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
    reset_password(schema, &account.identity, &account.state, new).await;
}

#[tokio::test]
async fn a_reset_signed_by_the_recovery_key_replaces_the_password_and_ends_every_session() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 6).await;
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

#[tokio::test]
async fn someone_from_another_server_signs_in_directly_without_a_password() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let guest = ctx.enrolled("guest", 8).await;
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
    assert_eq!(
        body["loginWithKey"]["userId"],
        json!(guest.user.id.to_string())
    );
}

#[tokio::test]
async fn registration_is_refused_for_a_principal_an_account_here_holds() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::Open).await;
    let ada = signed_up(&ctx, "ada", 9).await;
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
