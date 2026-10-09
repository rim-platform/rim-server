#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;

#[tokio::test]
async fn create_get_by_id_and_token_hash() {
    let ctx = support::TestCtx::new().await;

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
    let ctx = support::TestCtx::new().await;

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

    // Already-revoked sessions keep their original revocation time and aren't recounted.
    let again = session_repo.revoke_all_for_user(u1.id).await.unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn revoke_all_for_user_except_keeps_one_session_and_other_users() {
    let ctx = support::TestCtx::new().await;

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());
    let ada = ctx.create_user("ada").await;
    let grace = ctx.create_user("grace").await;

    let session = |user_id, token_hash: &str| NewSession {
        user_id,
        token_hash: token_hash.to_string(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        user_agent: None,
        ip: None,
    };
    let current = session_repo
        .create_session(session(ada.id, "ada-current"))
        .await
        .unwrap();
    let laptop = session_repo
        .create_session(session(ada.id, "ada-laptop"))
        .await
        .unwrap();
    let phone = session_repo
        .create_session(session(ada.id, "ada-phone"))
        .await
        .unwrap();
    let already_revoked = session_repo
        .create_session(session(ada.id, "ada-old"))
        .await
        .unwrap();
    session_repo
        .revoke_session(already_revoked.id)
        .await
        .unwrap();
    let revoked_at = session_repo
        .get_by_id(already_revoked.id)
        .await
        .unwrap()
        .unwrap()
        .revoked_at;
    let other_user = session_repo
        .create_session(session(grace.id, "grace"))
        .await
        .unwrap();

    let ended = session_repo
        .revoke_all_for_user_except(ada.id, current.id)
        .await
        .unwrap();
    assert_eq!(ended, 2);

    let get = |id| {
        let repo = &session_repo;
        async move { repo.get_by_id(id).await.unwrap().unwrap() }
    };
    assert!(get(current.id).await.revoked_at.is_none());
    assert!(get(laptop.id).await.revoked_at.is_some());
    assert!(get(phone.id).await.revoked_at.is_some());
    // An already-revoked session keeps its original revocation time and isn't counted.
    assert_eq!(get(already_revoked.id).await.revoked_at, revoked_at);
    assert!(get(other_user.id).await.revoked_at.is_none());

    let again = session_repo
        .revoke_all_for_user_except(ada.id, current.id)
        .await
        .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn touch_last_seen_at_updates_timestamp() {
    let ctx = support::TestCtx::new().await;

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
