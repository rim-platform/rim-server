//! Resolver-level e2e test for `resetPassword` over a real database.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::Request;
use chrono::{Duration, Utc};

use rim_auth::password::verify_password;
use rim_auth::session_token::hash_token;
use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::password_reset::model::NewPasswordResetToken;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::password_reset_token::SeaOrmPasswordResetTokenRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;

fn reset_mutation(token: &str, new_password: &str) -> String {
    format!(r#"mutation {{ resetPassword(token: "{token}", newPassword: "{new_password}") }}"#)
}

/// A completed reset changes the password, revokes every existing session (an attacker's
/// stolen one included), and spends every other outstanding reset link for the account.
#[tokio::test]
async fn reset_password_revokes_sessions_and_outstanding_tokens() {
    let ctx = TestCtx::new().await;
    let user = ctx.create_user("reset-victim").await;

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());
    let mut session_ids = Vec::new();
    for hash in ["victim-session", "attacker-session"] {
        let session = session_repo
            .create_session(NewSession {
                user_id: user.id,
                token_hash: hash.to_string(),
                expires_at: Utc::now() + Duration::days(1),
                user_agent: None,
                ip: None,
            })
            .await
            .expect("create_session failed");
        session_ids.push(session.id);
    }

    let token_repo = SeaOrmPasswordResetTokenRepository::new(ctx.db.clone());
    for raw in ["rim_rst_used_now", "rim_rst_older_link"] {
        token_repo
            .create(NewPasswordResetToken {
                user_id: user.id,
                token_hash: hash_token(raw),
                expires_at: Utc::now() + Duration::hours(1),
            })
            .await
            .expect("create reset token failed");
    }

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let res = schema
        .execute(Request::new(reset_mutation(
            "rim_rst_used_now",
            "brand-new-password",
        )))
        .await;
    assert!(
        res.errors.is_empty(),
        "resetPassword failed: {:?}",
        res.errors
    );

    let user_after = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(user.id)
        .await
        .expect("user lookup failed")
        .expect("user exists");
    assert!(
        verify_password(
            "brand-new-password",
            user_after.password_hash.as_deref().expect("password hash"),
        )
        .unwrap()
    );

    for id in session_ids {
        let session = session_repo
            .get_by_id(id)
            .await
            .expect("session lookup failed")
            .expect("session exists");
        assert!(
            session.revoked_at.is_some(),
            "every pre-reset session must be revoked"
        );
    }

    let older = token_repo
        .get_by_token_hash(&hash_token("rim_rst_older_link"))
        .await
        .expect("token lookup failed")
        .expect("token exists");
    assert!(
        older.used_at.is_some(),
        "other outstanding reset links must be spent"
    );

    // Neither link works again.
    for raw in ["rim_rst_used_now", "rim_rst_older_link"] {
        let replay = schema
            .execute(Request::new(reset_mutation(raw, "another-password-1")))
            .await;
        assert!(!replay.errors.is_empty(), "{raw} must not be reusable");
    }
}
