//! Password login against credential changes on a live database: a login whose password check
//! raced a password reset, a key enrollment or a ban waits for the change to commit and then
//! leaves no live session behind.
//!
//! Each race holds the user row lock in a transaction that makes the change, starts a login with
//! the old password, waits until that login waits on the lock, and only then commits.
//!
//! Only compiled with `--features infra-db-tests`; needs `DATABASE_URL` (see `support`).

#![cfg(feature = "infra-db-tests")]

mod support;

use std::sync::Arc;

use chrono::Duration;
use rim_core::key_identity::model::{KeyLogEntryType, NewKeyLogEntry};
use rim_core::user::model::{NewUser, User, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_db::{DbHandle, begin_transaction};
use rim_infra_services::AppRepos;
use rim_infra_services::auth::DefaultAuthService;
use rim_services::auth::command::LoginCmd;
use rim_services::auth::dto::LoginResultDto;
use rim_services::auth::service::AuthService;
use rim_services::events::{NoopBroadcaster, Observers};
use rim_services::{ServiceError, ServiceResult};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use tokio::task::JoinHandle;

const OLD_PASSWORD: &str = "old-password-123";
const EMAIL: &str = "login-race@example.com";

fn build_auth_service(db: DatabaseConnection) -> DefaultAuthService {
    let repos = AppRepos::with_handle(&DbHandle::Connection(db.clone())).for_auth();
    DefaultAuthService::new(
        repos,
        Some(db),
        Duration::days(30),
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>),
        Arc::new(rim_infra_services::email::LogEmailSender),
        None,
    )
}

/// A clean database with one password account, `alice` / [`OLD_PASSWORD`], whose email is not
/// blocked.
async fn setup() -> (DatabaseConnection, User) {
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    let users = SeaOrmUserRepository::new(DbHandle::Connection(db.clone()));
    users.unblock_email(EMAIL).await.expect("unblock email");
    let alice = users
        .create_user(NewUser {
            username: "alice".to_string(),
            password: Some(rim_auth::password::hash_password(OLD_PASSWORD).expect("hash")),
            email: Some(EMAIL.to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("create alice");
    (db, alice)
}

/// How many backends of this database wait on a lock.
async fn lock_waiters(db: &DatabaseConnection) -> i64 {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        ))
        .await
        .expect("query pg_stat_activity")
        .expect("one row");
    row.try_get("", "n").expect("count")
}

/// Start a login to `alice` with [`OLD_PASSWORD`] and return once it waits on a lock, or once it
/// finished without waiting.
async fn start_login(db: &DatabaseConnection) -> JoinHandle<ServiceResult<LoginResultDto>> {
    let svc = build_auth_service(db.clone());
    let login = tokio::spawn(async move {
        svc.login(LoginCmd {
            username: "alice".to_string(),
            password: OLD_PASSWORD.to_string(),
            remote_ip: "203.0.113.7".to_string(),
        })
        .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    while !login.is_finished() && lock_waiters(db).await == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the login neither finished nor waited on a lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    login
}

/// How many of `user_id`'s sessions are neither revoked nor expired.
async fn live_sessions(db: &DatabaseConnection, user_id: UserId) -> i64 {
    let id: uuid::Uuid = user_id.into();
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM session \
             WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
            [id.into()],
        ))
        .await
        .expect("count sessions")
        .expect("one row");
    row.try_get("", "n").expect("count")
}

#[tokio::test]
async fn a_login_with_no_change_in_between_creates_one_live_session() {
    let (db, alice) = setup().await;
    let login = start_login(&db).await;
    let result = login.await.expect("login task");
    assert_eq!(result.expect("login succeeds").user_id, alice.id);
    assert_eq!(live_sessions(&db, alice.id).await, 1);
}

#[tokio::test]
async fn a_login_racing_a_password_reset_leaves_no_live_session() {
    let (db, alice) = setup().await;

    // The writes of a password reset, under the user row lock.
    let scope = begin_transaction(&db).await.expect("begin");
    let repos = AppRepos::with_handle(&scope.handle());
    assert!(repos.user_repo.lock_user(alice.id).await.expect("lock"));
    repos
        .user_repo
        .update_password(
            alice.id,
            rim_auth::password::hash_password("new-password-456").expect("hash"),
        )
        .await
        .expect("update password");
    repos
        .session_repo
        .revoke_all_for_user(alice.id)
        .await
        .expect("revoke sessions");

    let login = start_login(&db).await;
    assert!(!login.is_finished(), "the login waits for the reset");
    scope.commit().await.expect("commit");

    let result = login.await.expect("login task");
    assert!(
        matches!(result, Err(ServiceError::Unauthenticated)),
        "{result:?}"
    );
    assert_eq!(live_sessions(&db, alice.id).await, 0);
}

#[tokio::test]
async fn a_login_racing_a_key_enrollment_leaves_no_live_session() {
    let (db, alice) = setup().await;

    // The writes of a key enrollment, under the user row lock.
    let scope = begin_transaction(&db).await.expect("begin");
    let repos = AppRepos::with_handle(&scope.handle());
    assert!(repos.user_repo.lock_user(alice.id).await.expect("lock"));
    repos
        .key_identity_repo
        .enroll(
            alice.id,
            "rimp1_login_race",
            "identity-key-0",
            vec![NewKeyLogEntry {
                seq: 0,
                entry_type: KeyLogEntryType::Inception,
                payload: r#"{"seq":0}"#.to_string(),
                signature: "sig-0".to_string(),
            }],
        )
        .await
        .expect("enroll");
    repos
        .session_repo
        .revoke_all_for_user(alice.id)
        .await
        .expect("revoke sessions");

    let login = start_login(&db).await;
    assert!(!login.is_finished(), "the login waits for the enrollment");
    scope.commit().await.expect("commit");

    let result = login.await.expect("login task");
    assert!(
        matches!(result, Err(ServiceError::Unauthenticated)),
        "{result:?}"
    );
    assert_eq!(live_sessions(&db, alice.id).await, 0);
}

#[tokio::test]
async fn a_login_racing_a_ban_leaves_no_live_session() {
    let (db, alice) = setup().await;

    // The writes of an operator's ban, under the user row lock.
    let scope = begin_transaction(&db).await.expect("begin");
    let repos = AppRepos::with_handle(&scope.handle());
    assert!(repos.user_repo.lock_user(alice.id).await.expect("lock"));
    repos
        .session_repo
        .revoke_all_for_user(alice.id)
        .await
        .expect("revoke sessions");
    repos
        .user_repo
        .block_email(EMAIL, "spam")
        .await
        .expect("block email");

    let login = start_login(&db).await;
    assert!(!login.is_finished(), "the login waits for the ban");
    scope.commit().await.expect("commit");

    let result = login.await.expect("login task");
    assert!(
        matches!(result, Err(ServiceError::Forbidden(_))),
        "{result:?}"
    );
    assert_eq!(live_sessions(&db, alice.id).await, 0);

    SeaOrmUserRepository::new(DbHandle::Connection(db.clone()))
        .unblock_email(EMAIL)
        .await
        .expect("unblock email");
}
