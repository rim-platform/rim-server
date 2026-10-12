#![cfg(feature = "db-tests")]

mod support;

use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_and_get_user_roundtrip() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_roundtrip".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_roundtrip@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.username, "u_test_roundtrip");
    assert_eq!(
        fetched.email.as_deref(),
        Some("u_test_roundtrip@test.example.com")
    );
}

#[tokio::test]
async fn create_user_duplicate_username_is_conflict() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    // Schema enforces username uniqueness via a unique index.
    let _u1 = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_duplicate@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let err = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: Some("pw2".to_string()),
            email: Some("u_test_duplicate2@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap_err();

    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    ));
}

#[tokio::test]
async fn find_by_username() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    assert!(repo.find_by_username("missing").await.unwrap().is_none());

    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_find".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_find@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let found = repo.find_by_username("u_test_find").await.unwrap().unwrap();
    assert_eq!(found.id, created.id);
}

#[tokio::test]
async fn record_terms_acceptance_stores_the_url_and_time() {
    use sea_orm::EntityTrait;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let user = ctx.create_user("u_test_terms").await;
    let untouched = ctx.create_user("u_test_no_terms").await;
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    repo.record_terms_acceptance(user.id, "https://rim.example.org/terms".to_string(), at)
        .await
        .unwrap();

    let row = entity::user::Entity::find_by_id(sea_orm::prelude::Uuid::from(user.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terms_accepted_url.as_deref(),
        Some("https://rim.example.org/terms")
    );
    assert_eq!(row.terms_accepted_at.map(|t| t.to_utc()), Some(at));
    let row = entity::user::Entity::find_by_id(sea_orm::prelude::Uuid::from(untouched.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.terms_accepted_url, row.terms_accepted_at),
        (None, None)
    );

    let err = repo
        .record_terms_acceptance(
            rim_core::user::model::UserId::new(),
            "https://rim.example.org/terms".to_string(),
            at,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::NotFound
    ));
}
