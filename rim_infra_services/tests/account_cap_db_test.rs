//! The account cap on a live database: key signups racing for the last places admit exactly as
//! many accounts as the cap allows and refuse the rest as full, and the service then says
//! registration is full.

#![cfg(feature = "infra-db-tests")]

mod support;

use std::sync::Arc;

use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_services::ServiceError;
use rim_services::limits::InstanceLimits;
use rim_services::provider::AppServiceProvider;

/// The services over `db`, accepting at most `accounts` local accounts.
fn capped_provider(db: sea_orm::DatabaseConnection, accounts: u32) -> DefaultAppServiceProvider {
    let repos = AppRepos::with_handle(&DbHandle::Connection(db.clone()));
    support::provider(
        db,
        &repos,
        InstanceLimits {
            accounts: Some(accounts),
            ..Default::default()
        },
    )
}

/// Racer `n`'s key signup, from an address of its own.
async fn open_signup(
    provider: &DefaultAppServiceProvider,
    n: u8,
) -> rim_services::ServiceResult<()> {
    support::key_signup(
        provider,
        &format!("racer{n}"),
        n + 1,
        None,
        &format!("198.51.100.{n}"),
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn racing_signups_fill_the_cap_exactly_and_the_rest_are_refused_as_full() {
    const CAP: u32 = 3;
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    let provider = Arc::new(capped_provider(db.clone(), CAP));
    assert!(
        !provider
            .auth_service()
            .registration_full()
            .await
            .expect("full")
    );

    let mut tasks = Vec::new();
    for n in 0..8 {
        let provider = Arc::clone(&provider);
        tasks.push(tokio::spawn(async move { open_signup(&provider, n).await }));
    }
    let (mut admitted, mut full) = (0, 0);
    for task in tasks {
        match task.await.expect("join") {
            Ok(()) => admitted += 1,
            Err(ServiceError::RegistrationFull) => full += 1,
            Err(e) => panic!("unexpected refusal: {e}"),
        }
    }
    assert_eq!((admitted, full), (CAP, 5));
    let users = SeaOrmUserRepository::new(DbHandle::Connection(db.clone()));
    assert_eq!(users.count_local_accounts(100).await.expect("count"), 3);
    assert!(
        provider
            .auth_service()
            .registration_full()
            .await
            .expect("full")
    );

    // Services built afresh read the same: the database is the answer, not the process.
    let fresh = capped_provider(db.clone(), CAP + 1);
    assert!(
        !fresh
            .auth_service()
            .registration_full()
            .await
            .expect("full")
    );
    open_signup(&fresh, 9).await.expect("one more fits");
    assert!(
        fresh
            .auth_service()
            .registration_full()
            .await
            .expect("full")
    );
}
