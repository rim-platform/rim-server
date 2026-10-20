//! The account cap on a live database: signups racing for the last places admit exactly as
//! many accounts as the cap allows and refuse the rest as full, and the service then says
//! registration is full.

#![cfg(feature = "infra-db-tests")]

mod support;

use std::sync::Arc;

use chrono::Duration;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::AppRepos;
use rim_infra_services::auth::DefaultAuthService;
use rim_services::ServiceError;
use rim_services::auth::command::SignupOpenCmd;
use rim_services::auth::service::AuthService;
use rim_services::events::{NoopBroadcaster, Observers};
use rim_services::limits::InstanceLimits;

/// An auth service over `db` that accepts at most `accounts` local accounts.
fn capped_auth_service(db: sea_orm::DatabaseConnection, accounts: u32) -> DefaultAuthService {
    let repos = AppRepos::with_handle(&DbHandle::Connection(db.clone())).for_auth();
    DefaultAuthService::new(
        repos,
        Some(db),
        Duration::days(30),
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>),
        Arc::new(rim_infra_services::email::LogEmailSender),
        None,
    )
    .with_limits(InstanceLimits {
        accounts: Some(accounts),
        ..Default::default()
    })
}

fn open_signup(n: usize) -> SignupOpenCmd {
    SignupOpenCmd {
        username: format!("racer{n}"),
        password: "password1234".to_string(),
        remote_ip: format!("198.51.100.{n}"),
        email: format!("racer{n}@example.com"),
        display_name: None,
        terms_accepted_url: None,
        confirmed_minimum_age: None,
    }
}

#[tokio::test]
async fn racing_signups_fill_the_cap_exactly_and_the_rest_are_refused_as_full() {
    const CAP: u32 = 3;
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    let svc = Arc::new(capped_auth_service(db.clone(), CAP));
    assert!(!svc.registration_full().await.expect("full"));

    let mut tasks = Vec::new();
    for n in 0..8 {
        let svc = Arc::clone(&svc);
        tasks.push(tokio::spawn(async move {
            svc.signup_open(open_signup(n)).await
        }));
    }
    let (mut admitted, mut full) = (0, 0);
    for task in tasks {
        match task.await.expect("join") {
            Ok(_) => admitted += 1,
            Err(ServiceError::RegistrationFull) => full += 1,
            Err(e) => panic!("unexpected refusal: {e}"),
        }
    }
    assert_eq!((admitted, full), (CAP, 5));
    let users = SeaOrmUserRepository::new(DbHandle::Connection(db.clone()));
    assert_eq!(users.count_local_accounts(100).await.expect("count"), 3);
    assert!(svc.registration_full().await.expect("full"));

    // A service built afresh reads the same: the database is the answer, not the process.
    let fresh = capped_auth_service(db.clone(), CAP + 1);
    assert!(!fresh.registration_full().await.expect("full"));
    fresh
        .signup_open(open_signup(9))
        .await
        .expect("one more fits");
    assert!(fresh.registration_full().await.expect("full"));
}
