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
use support::identity::{signed_up, with_session};

const LOGOUT_OTHER_SESSIONS: &str = "mutation { logoutOtherSessions }";

const ME: &str = "query { me { id username } }";

const LOGOUT: &str = "mutation { logout }";

/// Two key sessions of one account and one of another: `logoutOtherSessions` from the first
/// ends the second, keeps the caller, and leaves the other account alone.
#[tokio::test]
async fn logout_other_sessions_ends_every_session_but_the_callers() {
    let ctx = TestCtx::new().await;
    let ada = signed_up(&ctx, "ada", 1).await;
    let grace = signed_up(&ctx, "grace", 2).await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    // The signup's own session ends, so only the two below are ada's.
    exec_ok(&schema, ada.caller.request(LOGOUT, json!({}))).await;

    let desktop = ada.sign_in(&schema).await;
    let laptop = ada.sign_in(&schema).await;
    let grace = grace.sign_in(&schema).await;

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

/// Without a session every session root is refused.
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
    let ada = signed_up(&ctx, "ada", 1).await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let desktop = ada.sign_in(&schema).await;
    let laptop = ada.sign_in(&schema).await;

    let data = exec_ok(&schema, with_session(&provider, &laptop, LOGOUT).await).await;
    assert_eq!(data["logout"], json!(true));

    for query in [ME, LOGOUT] {
        let request = with_session(&provider, &laptop, query).await;
        assert_eq!(exec_err_code(&schema, request).await, "UNAUTHENTICATED");
    }
    let data = exec_ok(&schema, with_session(&provider, &desktop, ME).await).await;
    assert_eq!(data["me"]["username"], json!("ada"));
}
