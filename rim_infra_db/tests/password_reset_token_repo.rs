#![cfg(feature = "db-tests")]

mod support;

use rim_core::password_reset::model::NewPasswordResetToken;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_infra_db::sea_orm_impl::password_reset_token::SeaOrmPasswordResetTokenRepository;

fn new_token(
    user_id: rim_core::user::model::UserId,
    hash: &str,
    expires_in: chrono::Duration,
) -> NewPasswordResetToken {
    NewPasswordResetToken {
        user_id,
        token_hash: hash.to_string(),
        expires_at: chrono::Utc::now() + expires_in,
    }
}

#[tokio::test]
async fn try_mark_used_claims_a_token_exactly_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPasswordResetTokenRepository::new(ctx.db.clone());
    let user = ctx.create_user("reset-once").await;

    let token = repo
        .create(new_token(user.id, "once-hash", chrono::Duration::hours(1)))
        .await
        .unwrap();

    assert!(repo.try_mark_used(token.id).await.unwrap());
    assert!(
        !repo.try_mark_used(token.id).await.unwrap(),
        "a used token must not be claimable again"
    );
    let stored = repo.get_by_token_hash("once-hash").await.unwrap().unwrap();
    assert!(stored.used_at.is_some());
}

#[tokio::test]
async fn try_mark_used_refuses_an_expired_token() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPasswordResetTokenRepository::new(ctx.db.clone());
    let user = ctx.create_user("reset-expired").await;

    let token = repo
        .create(new_token(
            user.id,
            "expired-hash",
            chrono::Duration::minutes(-1),
        ))
        .await
        .unwrap();

    assert!(!repo.try_mark_used(token.id).await.unwrap());
}

#[tokio::test]
async fn invalidate_unused_for_user_marks_only_that_users_open_tokens() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPasswordResetTokenRepository::new(ctx.db.clone());
    let alice = ctx.create_user("reset-alice").await;
    let bob = ctx.create_user("reset-bob").await;

    let used = repo
        .create(new_token(alice.id, "a-used", chrono::Duration::hours(1)))
        .await
        .unwrap();
    assert!(repo.try_mark_used(used.id).await.unwrap());
    repo.create(new_token(alice.id, "a-open-1", chrono::Duration::hours(1)))
        .await
        .unwrap();
    repo.create(new_token(alice.id, "a-open-2", chrono::Duration::hours(1)))
        .await
        .unwrap();
    repo.create(new_token(bob.id, "b-open", chrono::Duration::hours(1)))
        .await
        .unwrap();

    assert_eq!(repo.invalidate_unused_for_user(alice.id).await.unwrap(), 2);

    for hash in ["a-open-1", "a-open-2"] {
        let t = repo.get_by_token_hash(hash).await.unwrap().unwrap();
        assert!(t.used_at.is_some(), "{hash} should be invalidated");
    }
    let bob_token = repo.get_by_token_hash("b-open").await.unwrap().unwrap();
    assert!(
        bob_token.used_at.is_none(),
        "other users' tokens are untouched"
    );
}
