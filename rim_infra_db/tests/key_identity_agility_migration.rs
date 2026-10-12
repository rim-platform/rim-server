#![cfg(feature = "db-tests")]

//! The `key_identity_algorithm_agility` migration: stored keys move to the tagged form, and
//! `device_key.key_hash` (the SQL twin of `rim_identity::PublicKey::key_hash`) replaces the
//! unique index on the raw public key.
//!
//! Rolls the schema back to just before the migration, seeds rows the old way, and migrates up
//! again. Destructive for the shared test database, like every db-test: run single-threaded.

mod support;

use migration::{Migrator, MigratorTrait};
use rim_core::key_identity::model::NewDeviceKey;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_identity::testing::signing_key;
use rim_identity::{PublicKey, StatementVersion};
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261009_075648_key_identity_algorithm_agility";

/// Migration steps from the newest down to just before `key_identity_algorithm_agility`,
/// robust to later migrations being appended after it.
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

fn ed25519_key(seed: u8) -> PublicKey {
    PublicKey::from(&signing_key(seed))
}

async fn exec(conn: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn device_row(conn: &DatabaseConnection, device_id: Uuid) -> (String, Option<Vec<u8>>) {
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT public_key, key_hash FROM device_key WHERE device_id = $1",
            [device_id.into()],
        ))
        .await
        .expect("select device")
        .expect("device row");
    (
        row.try_get("", "public_key").expect("public_key"),
        row.try_get("", "key_hash").expect("key_hash"),
    )
}

async fn identity_key(conn: &DatabaseConnection, user_id: Uuid) -> String {
    conn.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"SELECT identity_key FROM "user" WHERE id = $1"#,
        [user_id.into()],
    ))
    .await
    .expect("select user")
    .expect("user row")
    .try_get("", "identity_key")
    .expect("identity_key")
}

#[tokio::test]
async fn existing_keys_are_tagged_and_hashed_and_down_restores_them() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    // Created on the current schema: the repository writes every column the entity has.
    let alice = ctx.create_user("alice").await;
    let alice_id: Uuid = alice.id.into();

    Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect("roll back to before key_identity_algorithm_agility");

    // An account enrolled before the migration: bare Ed25519 keys, as the first key identity
    // release stored them.
    let identity = ed25519_key(1);
    let laptop_key = ed25519_key(20);
    let phone_key = ed25519_key(21);
    let (laptop, phone) = (Uuid::now_v7(), Uuid::now_v7());
    exec(
        &conn,
        r#"UPDATE "user" SET principal_id = 'rimp1_alice', identity_key = $2 WHERE id = $1"#,
        vec![
            alice_id.into(),
            identity.encode_for(StatementVersion::V1).into(),
        ],
    )
    .await;
    for (device_id, key) in [(laptop, &laptop_key), (phone, &phone_key)] {
        exec(
            &conn,
            "INSERT INTO device_key (id, user_id, device_id, public_key, name, cert_payload, \
             cert_signature, key_seq) VALUES ($1, $2, $3, $4, 'Device', '{}', 'sig', 0)",
            vec![
                Uuid::now_v7().into(),
                alice_id.into(),
                device_id.into(),
                key.encode_for(StatementVersion::V1).into(),
            ],
        )
        .await;
    }

    Migrator::up(&conn, None)
        .await
        .expect("re-apply key_identity_algorithm_agility");

    assert_eq!(identity_key(&conn, alice_id).await, identity.to_string());
    for (device_id, key) in [(laptop, &laptop_key), (phone, &phone_key)] {
        let (public_key, key_hash) = device_row(&conn, device_id).await;
        assert_eq!(public_key, key.to_string());
        // The SQL hash is the one the server computes for new devices.
        assert_eq!(key_hash, Some(key.key_hash().to_vec()));
    }

    // The new unique index refuses the same key again, under a new device id.
    let repo = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let err = repo
        .add_device(NewDeviceKey {
            user_id: alice.id,
            device_id: Uuid::now_v7(),
            public_key: laptop_key.to_string(),
            key_hash: laptop_key.key_hash().to_vec(),
            name: "Again".to_string(),
            cert_payload: "{}".to_string(),
            cert_signature: "sig".to_string(),
            key_seq: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    // Down puts the bare keys and the raw-key index back.
    Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect("roll back again");
    assert_eq!(
        identity_key(&conn, alice_id).await,
        identity.encode_for(StatementVersion::V1)
    );
    let public_key: String = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT public_key FROM device_key WHERE device_id = $1",
            [laptop.into()],
        ))
        .await
        .expect("select")
        .expect("row")
        .try_get("", "public_key")
        .expect("public_key");
    assert_eq!(public_key, laptop_key.encode_for(StatementVersion::V1));

    Migrator::up(&conn, None).await.expect("migrate up again");
}

#[tokio::test]
async fn down_refuses_while_a_non_ed25519_key_is_stored() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let alice = ctx.create_user("alice").await;
    let device_id = Uuid::now_v7();
    // A key of an algorithm the previous schema cannot hold.
    exec(
        &conn,
        "INSERT INTO device_key (id, user_id, device_id, public_key, key_hash, name, \
         cert_payload, cert_signature, key_seq) \
         VALUES ($1, $2, $3, 'ed25519-mldsa44:AAAA', sha256('x'::bytea), 'D', '{}', 'sig', 0)",
        vec![
            Uuid::now_v7().into(),
            Uuid::from(alice.id).into(),
            device_id.into(),
        ],
    )
    .await;

    let err = Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect_err("down must refuse");
    assert!(err.to_string().contains("non-Ed25519"), "{err}");

    exec(
        &conn,
        "DELETE FROM device_key WHERE device_id = $1",
        vec![device_id.into()],
    )
    .await;
    // Whatever down managed to undo before refusing, bring the schema back to the head.
    Migrator::up(&conn, None).await.expect("migrate up again");
}
