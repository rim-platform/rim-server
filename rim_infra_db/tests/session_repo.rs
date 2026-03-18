#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_get_by_id_and_token_hash() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let session_repo = SeaOrmSessionRepository::new(db.clone());

    let user = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();

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
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let session_repo = SeaOrmSessionRepository::new(db.clone());

    // Missing id is Ok.
    session_repo
        .touch_last_seen_at(rim_core::auth::model::SessionId::new())
        .await
        .unwrap();
    session_repo
        .revoke_session(rim_core::auth::model::SessionId::new())
        .await
        .unwrap();

    let u1 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u1".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();
    let u2 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u2".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();

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
