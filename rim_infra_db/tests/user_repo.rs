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
            password: "pw".to_string(),
            email: "u_test_roundtrip@test.example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.username, "u_test_roundtrip");
    assert_eq!(fetched.email, "u_test_roundtrip@test.example.com");
}

#[tokio::test]
async fn create_user_duplicate_username_is_conflict() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    // Schema enforces username uniqueness via a unique index.
    let _u1 = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: "pw".to_string(),
            email: "u_test_duplicate@test.example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();

    let err = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: "pw2".to_string(),
            email: "u_test_duplicate2@test.example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap_err();

    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    ));
}

#[tokio::test]
async fn find_by_username_and_delete_by_username() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    assert!(repo.find_by_username("missing").await.unwrap().is_none());

    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_find".to_string(),
            password: "pw".to_string(),
            email: "u_test_find@test.example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();

    let found = repo.find_by_username("u_test_find").await.unwrap().unwrap();
    assert_eq!(found.id, created.id);

    repo.delete_by_username("u_test_find").await.unwrap();
    assert!(
        repo.find_by_username("u_test_find")
            .await
            .unwrap()
            .is_none()
    );
}
