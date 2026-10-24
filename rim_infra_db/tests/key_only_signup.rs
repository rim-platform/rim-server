#![cfg(feature = "db-tests")]

//! Key-only accounts: `user.email` is nullable, a live row needs a principal id, and the
//! `key_only_signup` migration's `down` refuses while a key-only account exists. Destructive for
//! the shared test database, like every db-test: run single-threaded.

mod support;

use migration::{Migrator, MigratorTrait};
use rim_core::key_identity::model::{KeyLogEntryType, NewKeyLogEntry};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::{NewUser, NewUserKeyIdentity};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const MIGRATION_NAME: &str = "m20261009_105834_key_only_signup";

/// Migration steps from the newest down to just before `key_only_signup`, robust to later
/// migrations being appended after it.
fn steps_to_before_migration() -> u32 {
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    u32::try_from(names.len() - index).expect("step count fits u32")
}

fn inception() -> NewKeyLogEntry {
    NewKeyLogEntry {
        seq: 0,
        entry_type: KeyLogEntryType::Inception,
        payload: r#"{"seq":0}"#.to_string(),
        signature: "sig-0".to_string(),
    }
}

fn key_only(username: &str, principal_id: &str) -> NewUser {
    NewUser {
        username: username.to_string(),
        email: None,
        display_name: Some(username.to_string()),
        key_identity: Some(NewUserKeyIdentity {
            principal_id: principal_id.to_string(),
            identity_key: "ed25519:identity".to_string(),
        }),
    }
}

#[tokio::test]
async fn a_key_only_account_round_trips_with_its_log() {
    let ctx = support::TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let keys = SeaOrmKeyIdentityRepository::new(ctx.db.clone());

    let user = users
        .create_user(key_only("ada", "rimp1_ada"))
        .await
        .unwrap();
    assert_eq!(user.email, None);
    assert_eq!(user.principal_id.as_deref(), Some("rimp1_ada"));
    assert_eq!(
        keys.find_user_by_principal_id("rimp1_ada").await.unwrap(),
        Some(user.id)
    );

    keys.insert_key_log(user.id, vec![inception()])
        .await
        .unwrap();
    assert_eq!(keys.list_key_log(user.id).await.unwrap().len(), 1);
    // The log is stored once.
    let err = keys
        .insert_key_log(user.id, vec![inception()])
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    // Two accounts without an email do not collide on the email index.
    users
        .create_user(key_only("grace", "rimp1_grace"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_principal_id_is_held_by_one_account() {
    let ctx = support::TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    users
        .create_user(key_only("ada", "rimp1_same"))
        .await
        .unwrap();
    let err = users
        .create_user(key_only("ada2", "rimp1_same"))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
}

#[tokio::test]
async fn a_live_account_needs_a_principal() {
    let ctx = support::TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let err = users
        .create_user(NewUser {
            key_identity: None,
            ..key_only("nobody", "unused")
        })
        .await
        .unwrap_err();
    // The CHECK constraint refuses it; no caller builds such a row on purpose.
    assert_eq!(err.kind(), RepoErrorKind::Internal);
}

#[tokio::test]
async fn down_refuses_while_a_key_only_account_exists() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let user = ctx.create_key_only_user("ada", "rimp1_ada").await;

    let err = Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect_err("down must refuse");
    assert!(err.to_string().contains("without a password"), "{err}");

    // Without one, down restores NOT NULL, and up lifts it again.
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"DELETE FROM "user" WHERE id = $1"#,
        [uuid::Uuid::from(user.id).into()],
    ))
    .await
    .expect("delete key-only account");
    // The refused down left this migration the newest applied one; undo just it.
    let applied = Migrator::get_applied_migrations(&conn)
        .await
        .expect("list applied migrations");
    assert_eq!(
        applied.last().map(|m| m.name().to_string()).as_deref(),
        Some(MIGRATION_NAME)
    );
    Migrator::down(&conn, Some(1))
        .await
        .expect("down without key-only accounts");
    let refused = conn
        .execute_unprepared(
            r#"INSERT INTO "user" (id, username, password_hash, email)
               VALUES (gen_random_uuid(), 'no_email', 'hash', NULL)"#,
        )
        .await;
    assert!(refused.is_err(), "email must be NOT NULL again");
    Migrator::up(&conn, None).await.expect("migrate up again");
}
