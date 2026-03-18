#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;

#[tokio::test]
async fn create_get_by_id_and_token_hash() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());
    let user = ctx.create_user("u").await;

    let s = session_repo
        .create_session(NewSession {
            user_id: user.id,
            token_hash: "th".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();

    assert!(session_repo.get_by_id(s.id).await.unwrap().is_some());
    let by_hash = session_repo.get_by_token_hash("th").await.unwrap().unwrap();
    assert_eq!(by_hash.id, s.id);
    assert!(
        session_repo
            .get_by_token_hash("missing")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn touch_and_revoke_are_idempotent_and_revoke_all_counts() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());

    // Missing id is Ok.
    session_repo
        .touch_last_seen_at(rim_core::auth::model::SessionId::new())
        .await
        .unwrap();
    session_repo
        .revoke_session(rim_core::auth::model::SessionId::new())
        .await
        .unwrap();

    let u1 = ctx.create_user("u1").await;
    let u2 = ctx.create_user("u2").await;

    let _ = session_repo
        .create_session(NewSession {
            user_id: u1.id,
            token_hash: "th1".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();
    let _ = session_repo
        .create_session(NewSession {
            user_id: u1.id,
            token_hash: "th2".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();
    let _ = session_repo
        .create_session(NewSession {
            user_id: u2.id,
            token_hash: "th3".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();

    let affected = session_repo.revoke_all_for_user(u1.id).await.unwrap();
    assert_eq!(affected, 2);
}

#[tokio::test]
async fn touch_last_seen_at_updates_timestamp() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());
    let user = ctx.create_user("touch-user").await;

    let session = session_repo
        .create_session(NewSession {
            user_id: user.id,
            token_hash: "touch-hash".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();

    // Freshly created session should have no last_seen_at
    let fresh = session_repo
        .get_by_token_hash("touch-hash")
        .await
        .unwrap()
        .unwrap();
    assert!(fresh.last_seen_at.is_none());

    let before_touch = chrono::Utc::now();
    session_repo.touch_last_seen_at(session.id).await.unwrap();

    let touched = session_repo
        .get_by_token_hash("touch-hash")
        .await
        .unwrap()
        .unwrap();
    assert!(touched.last_seen_at.is_some());
    let last_seen = touched.last_seen_at.unwrap();
    assert!(last_seen >= before_touch - chrono::Duration::seconds(1));
    assert!(last_seen <= chrono::Utc::now() + chrono::Duration::seconds(1));
}
