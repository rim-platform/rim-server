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
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

use rim_api::auth_context::RemoteIp;
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
use support::graphql::{exec_err_code, exec_ok};

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
        let replay = Request::new(reset_mutation(raw, "another-password-1"));
        assert_eq!(exec_err_code(&schema, replay).await, "FORBIDDEN", "{raw}");
    }
}

fn request_reset(username: &str, ip: &str) -> Request {
    Request::new(format!(
        r#"mutation {{ requestPasswordReset(username: "{username}") }}"#
    ))
    .data(RemoteIp(ip.to_string()))
}

/// The reset links stored for `user_id`, once the background insert has run.
async fn reset_links(ctx: &TestCtx, user_id: rim_core::user::model::UserId, expected: u64) -> u64 {
    let user_id: uuid::Uuid = user_id.into();
    let mut count = 0;
    for _ in 0..50 {
        count = entity::password_reset_token::Entity::find()
            .filter(entity::password_reset_token::Column::UserId.eq(user_id))
            .count(&ctx.conn)
            .await
            .expect("count reset links");
        if count >= expected {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    count
}

/// A known and an unknown username get the same answer; only the known one gets a link. One
/// address gets ten answers an hour, then `RATE_LIMITED`.
#[tokio::test]
async fn request_password_reset_answers_alike_for_any_username_and_limits_an_address() {
    let ctx = TestCtx::new().await;
    let user = ctx.create_user("reset-known").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    for username in ["reset-known", "reset-unknown"] {
        let data = exec_ok(&schema, request_reset(username, "192.0.2.1")).await;
        assert_eq!(data["requestPasswordReset"], true, "{username}");
    }
    assert_eq!(reset_links(&ctx, user.id, 1).await, 1);

    for i in 0..10 {
        let data = exec_ok(&schema, request_reset(&format!("anyone-{i}"), "192.0.2.2")).await;
        assert_eq!(data["requestPasswordReset"], true);
    }
    let refused = request_reset("anyone-10", "192.0.2.2");
    assert_eq!(exec_err_code(&schema, refused).await, "RATE_LIMITED");
}

/// An unknown link is `NOT_FOUND`, an expired one `FORBIDDEN`, and a short password `VALIDATION`
/// that leaves the link usable.
#[tokio::test]
async fn reset_password_refuses_an_unknown_or_expired_link_and_a_short_password() {
    let ctx = TestCtx::new().await;
    let user = ctx.create_user("reset-refused").await;
    let token_repo = SeaOrmPasswordResetTokenRepository::new(ctx.db.clone());
    for (raw, expires_in) in [
        ("rim_rst_live", Duration::hours(1)),
        ("rim_rst_old", Duration::hours(-1)),
    ] {
        token_repo
            .create(NewPasswordResetToken {
                user_id: user.id,
                token_hash: hash_token(raw),
                expires_at: Utc::now() + expires_in,
            })
            .await
            .expect("create reset token failed");
    }
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    for (raw, password, code) in [
        ("rim_rst_unknown", "long-enough-password", "NOT_FOUND"),
        ("rim_rst_old", "long-enough-password", "FORBIDDEN"),
        ("rim_rst_live", "short", "VALIDATION"),
    ] {
        let request = Request::new(reset_mutation(raw, password));
        assert_eq!(exec_err_code(&schema, request).await, code, "{raw}");
    }
    let data = exec_ok(
        &schema,
        Request::new(reset_mutation("rim_rst_live", "long-enough-password")),
    )
    .await;
    assert_eq!(data["resetPassword"], true);
}
