#![cfg(feature = "db-tests")]

mod support;

use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_and_get_user_roundtrip() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let repo = SeaOrmUserRepository::new(db.clone());
    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_roundtrip".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.username, "u_test_roundtrip");
}

#[tokio::test]
async fn create_user_duplicate_username_is_conflict() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let repo = SeaOrmUserRepository::new(db.clone());

    // Schema enforces username uniqueness via a unique index.
    let _u1 = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();

    let err = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: "pw2".to_string(),
        })
        .await
        .unwrap_err();

    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    ));
}

#[tokio::test]
async fn find_by_username_and_delete_by_username_and_any_users_exist() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let repo = SeaOrmUserRepository::new(db.clone());

    assert_eq!(repo.any_users_exist().await.unwrap(), false);
    assert!(repo.find_by_username("missing").await.unwrap().is_none());

    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_find".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(repo.any_users_exist().await.unwrap(), true);
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
