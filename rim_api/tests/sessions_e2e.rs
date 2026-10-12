//! Resolver-level e2e test for `logoutOtherSessions` over a real database.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::json;

use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{anonymous_request, assert_each_refused, exec_err_code, exec_ok};
use support::sessions::{LOGIN, PASSWORD, login, with_session};

const LOGOUT_OTHER_SESSIONS: &str = "mutation { logoutOtherSessions }";

const ME: &str = "query { me { id username } }";

const LOGOUT: &str = "mutation { logout }";

/// Two password sessions of one account and one of another: `logoutOtherSessions` from the
/// first ends the second, keeps the caller, and leaves the other account alone.
#[tokio::test]
async fn logout_other_sessions_ends_every_session_but_the_callers() {
    let ctx = TestCtx::new().await;
    ctx.create_password_user("ada").await;
    ctx.create_password_user("grace").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();

    let desktop = login(&schema, "ada").await;
    let laptop = login(&schema, "ada").await;
    let grace = login(&schema, "grace").await;

    let data = exec_ok(
        &schema,
        with_session(&provider, &desktop, LOGOUT_OTHER_SESSIONS).await,
    )
    .await;
    assert_eq!(data["logoutOtherSessions"], json!(1));

    assert_eq!(
        exec_err_code(&schema, with_session(&provider, &laptop, ME).await).await,
        "UNAUTHENTICATED"
    );
    let data = exec_ok(&schema, with_session(&provider, &desktop, ME).await).await;
    assert_eq!(data["me"]["username"], json!("ada"));
    let data = exec_ok(&schema, with_session(&provider, &grace, ME).await).await;
    assert_eq!(data["me"]["username"], json!("grace"));

    // Nothing left to end.
    let data = exec_ok(
        &schema,
        with_session(&provider, &desktop, LOGOUT_OTHER_SESSIONS).await,
    )
    .await;
    assert_eq!(data["logoutOtherSessions"], json!(0));
}

/// Without a session every session root but `login` is refused.
#[tokio::test]
async fn every_session_root_needs_a_session() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let calls = [LOGOUT_OTHER_SESSIONS, LOGOUT, ME].map(|query| (query, json!({})));
    assert_each_refused(&schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// `logout` ends the calling session only: its cookie no longer authenticates, even to log out
/// again, and the account's other session goes on.
#[tokio::test]
async fn logout_ends_only_the_calling_session() {
    let ctx = TestCtx::new().await;
    ctx.create_password_user("ada").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let desktop = login(&schema, "ada").await;
    let laptop = login(&schema, "ada").await;

    let data = exec_ok(&schema, with_session(&provider, &laptop, LOGOUT).await).await;
    assert_eq!(data["logout"], json!(true));

    for query in [ME, LOGOUT] {
        let request = with_session(&provider, &laptop, query).await;
        assert_eq!(exec_err_code(&schema, request).await, "UNAUTHENTICATED");
    }
    let data = exec_ok(&schema, with_session(&provider, &desktop, ME).await).await;
    assert_eq!(data["me"]["username"], json!("ada"));
}

/// A wrong password and an unknown username get the same answer.
#[tokio::test]
async fn login_answers_a_wrong_password_like_an_unknown_user() {
    let ctx = TestCtx::new().await;
    ctx.create_password_user("ada").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let calls =
        [("ada", "wrong horse battery"), ("nobody", PASSWORD)].map(|(username, password)| {
            (
                LOGIN,
                json!({ "input": { "username": username, "password": password } }),
            )
        });
    assert_each_refused(&schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}
