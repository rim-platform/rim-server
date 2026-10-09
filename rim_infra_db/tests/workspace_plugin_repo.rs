#![cfg(feature = "db-tests")]

//! `workspace_plugin` and `plugin_bundle` storage: entries, the entry cap, bundles and their
//! cleanup, the tables' constraints and cascades, and the `add_workspace_plugin` migration.

mod support;

use chrono::Duration;
use migration::{Migrator, MigratorTrait};
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_plugin::model::{
    NewPluginBundle, PluginRuntimeKind, PluginSource, StatementText, WorkspacePluginId,
    WorkspacePluginStatus, WorkspacePluginWrite,
};
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use rim_infra_db::sea_orm_impl::workspace_plugin::SeaOrmWorkspacePluginRepository;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261009_133359_add_workspace_plugin";
const DAY: Duration = Duration::hours(24);

fn sha(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn bundle_source(n: u8) -> PluginSource {
    PluginSource::Bundle {
        url: format!("/api/plugin-bundles/x/{}", sha(n)),
        sha256: sha(n),
        size: 1605,
    }
}

fn git_source() -> PluginSource {
    PluginSource::Git {
        url: "https://github.com/acme/k8s-pods".to_string(),
        commit: "a".repeat(40),
        tree_sha256: sha(9),
    }
}

fn statement(n: u8) -> StatementText {
    StatementText {
        payload: format!(r#"{{"n":{n}}}"#),
        signature: format!("ed25519:sig{n}"),
    }
}

fn write(
    server_id: ServerId,
    plugin_id: &str,
    approver: UserId,
    source: PluginSource,
) -> WorkspacePluginWrite {
    let runtime_kind = match source {
        PluginSource::Bundle { .. } => PluginRuntimeKind::WebSandboxed,
        PluginSource::Git { .. } => PluginRuntimeKind::DesktopLocalProcess,
    };
    WorkspacePluginWrite {
        server_id,
        plugin_id: plugin_id.to_string(),
        version: "0.1.0".to_string(),
        display_name: "Hello".to_string(),
        runtime_kind,
        status: WorkspacePluginStatus::Recommended,
        source,
        statement: statement(1),
        publisher_principal_id: "rimp1_publisher".to_string(),
        publisher_key_seq: 0,
        publisher_key_log: None,
        approved_by: approver,
    }
}

fn new_bundle(server_id: ServerId, n: u8, uploader: UserId) -> NewPluginBundle {
    NewPluginBundle {
        server_id,
        sha256: sha(n),
        size_bytes: 1000 + u64::from(n),
        storage_key: format!("plugin-bundles/{server_id}/{}", sha(n)),
        uploaded_by: uploader,
    }
}

async fn exec(conn: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"))
    .rows_affected()
}

async fn age_bundle(conn: &DatabaseConnection, server_id: ServerId, n: u8, hours: i64) {
    exec(
        conn,
        "UPDATE plugin_bundle SET created_at = now() - make_interval(hours => $3) \
         WHERE server_id = $1 AND sha256 = $2",
        vec![
            Uuid::from(server_id).into(),
            sha(n).into(),
            (hours as i32).into(),
        ],
    )
    .await;
}

#[tokio::test]
async fn entries_round_trip_and_list_by_plugin_id() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let other = ctx.create_server("other").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());

    let mut web = write(server.id, "dev.example.zeta", owner.id, bundle_source(1));
    web.publisher_key_log = Some(vec![statement(7), statement(8)]);
    web.status = WorkspacePluginStatus::Required;
    let stored = repo
        .insert(web.clone(), 100)
        .await
        .unwrap()
        .expect("inserted");
    assert_eq!(stored.plugin_id, "dev.example.zeta");
    assert_eq!(stored.source, bundle_source(1));
    assert_eq!(stored.status, WorkspacePluginStatus::Required);
    assert_eq!(stored.runtime_kind, PluginRuntimeKind::WebSandboxed);
    assert_eq!(stored.publisher_key_log, web.publisher_key_log);
    assert_eq!(stored.approved_by, Some(owner.id));
    assert_eq!(stored.statement, statement(1));

    let desktop = write(server.id, "dev.example.alpha", owner.id, git_source());
    repo.insert(desktop, 100).await.unwrap().expect("inserted");
    repo.insert(
        write(other.id, "dev.example.beta", owner.id, git_source()),
        100,
    )
    .await
    .unwrap()
    .expect("inserted");

    let listed = repo.list(server.id).await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|e| e.plugin_id.as_str()).collect();
    assert_eq!(ids, ["dev.example.alpha", "dev.example.zeta"]);
    assert_eq!(listed[0].source, git_source());
    assert_eq!(listed[0].publisher_key_log, None);

    let got = repo.get(server.id, "dev.example.zeta").await.unwrap();
    assert_eq!(got, Some(stored));
    assert_eq!(repo.get(other.id, "dev.example.zeta").await.unwrap(), None);
}

#[tokio::test]
async fn a_plugin_id_is_in_a_registry_once() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    let entry = write(server.id, "dev.example.hello", owner.id, bundle_source(1));
    repo.insert(entry.clone(), 100)
        .await
        .unwrap()
        .expect("first");
    let err = repo.insert(entry, 100).await.expect_err("duplicate");
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
}

#[tokio::test]
async fn insert_stops_at_the_cap_per_server() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let other = ctx.create_server("other").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    for id in ["dev.example.a", "dev.example.b"] {
        repo.insert(write(server.id, id, owner.id, git_source()), 2)
            .await
            .unwrap()
            .expect("under the cap");
    }
    let full = repo
        .insert(write(server.id, "dev.example.c", owner.id, git_source()), 2)
        .await
        .unwrap();
    assert_eq!(full, None);
    assert_eq!(repo.list(server.id).await.unwrap().len(), 2);
    // Another workspace has its own cap.
    assert!(
        repo.insert(write(other.id, "dev.example.c", owner.id, git_source()), 2)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn update_replaces_the_written_columns_and_approval() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let moderator = ctx.create_user("moderator").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    let mut entry = write(server.id, "dev.example.hello", owner.id, bundle_source(1));
    entry.publisher_key_log = Some(vec![statement(7)]);
    let stored = repo.insert(entry, 100).await.unwrap().expect("inserted");

    let mut next = write(server.id, "dev.example.hello", moderator.id, git_source());
    next.version = "0.2.0".to_string();
    next.statement = statement(2);
    next.publisher_key_seq = 3;
    let updated = repo
        .update(stored.id, next)
        .await
        .unwrap()
        .expect("updated");
    assert_eq!(updated.id, stored.id);
    assert_eq!(updated.version, "0.2.0");
    assert_eq!(updated.source, git_source());
    assert_eq!(updated.runtime_kind, PluginRuntimeKind::DesktopLocalProcess);
    assert_eq!(updated.statement, statement(2));
    assert_eq!(updated.publisher_key_seq, 3);
    assert_eq!(updated.publisher_key_log, None);
    assert_eq!(updated.approved_by, Some(moderator.id));
    assert!(updated.approved_at >= stored.approved_at);
    assert!(updated.updated_at >= stored.updated_at);
    assert_eq!(updated.created_at, stored.created_at);

    let missing = repo
        .update(
            WorkspacePluginId::new(),
            write(server.id, "dev.example.hello", owner.id, git_source()),
        )
        .await
        .unwrap();
    assert_eq!(missing, None);
}

#[tokio::test]
async fn delete_removes_one_entry() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, git_source()),
        100,
    )
    .await
    .unwrap();
    assert!(repo.delete(server.id, "dev.example.hello").await.unwrap());
    assert!(!repo.delete(server.id, "dev.example.hello").await.unwrap());
    assert_eq!(
        repo.get(server.id, "dev.example.hello").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn bundles_are_recorded_once_per_hash() {
    let ctx = support::TestCtx::new().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let other = ctx.create_server("other").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());

    let first = repo
        .insert_bundle(new_bundle(server.id, 1, owner.id))
        .await
        .unwrap()
        .expect("new");
    assert_eq!(first.sha256, sha(1));
    assert_eq!(first.size_bytes, 1001);
    assert_eq!(first.uploaded_by, Some(owner.id));
    assert_eq!(
        repo.insert_bundle(new_bundle(server.id, 1, owner.id))
            .await
            .unwrap(),
        None
    );
    repo.insert_bundle(new_bundle(server.id, 2, owner.id))
        .await
        .unwrap()
        .expect("new");
    repo.insert_bundle(new_bundle(other.id, 1, owner.id))
        .await
        .unwrap()
        .expect("another workspace");

    assert_eq!(
        repo.get_bundle(server.id, &sha(1)).await.unwrap(),
        Some(first)
    );
    assert_eq!(repo.get_bundle(server.id, &sha(3)).await.unwrap(), None);
    assert_eq!(repo.bundle_bytes(server.id).await.unwrap(), 1001 + 1002);
    assert_eq!(repo.bundle_bytes(ServerId::new()).await.unwrap(), 0);
}

#[tokio::test]
async fn cleanup_deletes_only_unnamed_bundles_over_a_day_old() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let other = ctx.create_server("other").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    for n in 1..=3 {
        repo.insert_bundle(new_bundle(server.id, n, owner.id))
            .await
            .unwrap();
    }
    repo.insert_bundle(new_bundle(other.id, 1, owner.id))
        .await
        .unwrap();
    // 1: old and unnamed. 2: old, named by an entry. 3: unnamed but recent.
    age_bundle(&conn, server.id, 1, 25).await;
    age_bundle(&conn, server.id, 2, 25).await;
    age_bundle(&conn, other.id, 1, 25).await;
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, bundle_source(2)),
        100,
    )
    .await
    .unwrap();
    // Another workspace's entry naming hash 1 does not keep this workspace's copy.
    repo.insert(
        write(other.id, "dev.example.hello", owner.id, bundle_source(1)),
        100,
    )
    .await
    .unwrap();

    let deleted = repo.delete_unnamed_bundles(server.id, DAY).await.unwrap();
    assert_eq!(
        deleted,
        [format!("plugin-bundles/{}/{}", server.id, sha(1))]
    );
    assert_eq!(repo.get_bundle(server.id, &sha(1)).await.unwrap(), None);
    assert!(repo.get_bundle(server.id, &sha(2)).await.unwrap().is_some());
    assert!(repo.get_bundle(server.id, &sha(3)).await.unwrap().is_some());
    assert!(repo.get_bundle(other.id, &sha(1)).await.unwrap().is_some());
    assert!(
        repo.delete_unnamed_bundles(server.id, DAY)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn deleting_a_workspace_deletes_its_registry_and_bundles() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, git_source()),
        100,
    )
    .await
    .unwrap();
    repo.insert_bundle(new_bundle(server.id, 1, owner.id))
        .await
        .unwrap();
    exec(
        &conn,
        "DELETE FROM server WHERE id = $1",
        vec![Uuid::from(server.id).into()],
    )
    .await;
    assert!(repo.list(server.id).await.unwrap().is_empty());
    assert_eq!(repo.get_bundle(server.id, &sha(1)).await.unwrap(), None);
}

#[tokio::test]
async fn deleting_the_approver_or_uploader_keeps_the_rows() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, git_source()),
        100,
    )
    .await
    .unwrap();
    repo.insert_bundle(new_bundle(server.id, 1, owner.id))
        .await
        .unwrap();
    exec(
        &conn,
        r#"DELETE FROM "user" WHERE id = $1"#,
        vec![Uuid::from(owner.id).into()],
    )
    .await;
    let entry = repo
        .get(server.id, "dev.example.hello")
        .await
        .unwrap()
        .expect("kept");
    assert_eq!(entry.approved_by, None);
    let bundle = repo
        .get_bundle(server.id, &sha(1))
        .await
        .unwrap()
        .expect("kept");
    assert_eq!(bundle.uploaded_by, None);
}

#[tokio::test]
async fn the_tables_check_their_columns() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let server = ctx.create_server("acme").await;
    let server_id = Uuid::from(server.id);

    let insert_entry = |runtime_kind: &'static str,
                        status: &'static str,
                        source_kind: &'static str,
                        with_git: bool,
                        with_bundle: bool,
                        key_seq: i32| {
        let conn = conn.clone();
        async move {
            let git = |v: &str| with_git.then(|| v.to_string());
            let bundle = |v: &str| with_bundle.then(|| v.to_string());
            conn.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspace_plugin (id, server_id, plugin_id, version, display_name, \
                     runtime_kind, status, source_kind, git_url, git_commit, git_tree_sha256, \
                     bundle_url, bundle_sha256, bundle_size, statement_payload, \
                     statement_signature, publisher_principal_id, publisher_key_seq, approved_at) \
                 VALUES ($1, $2, $3, '1', 'x', $4, $5, $6, $7, $8, $9, $10, $11, $12, \
                     '{}', 's', 'p', $13, now())",
                [
                    Uuid::now_v7().into(),
                    server_id.into(),
                    Uuid::now_v7().to_string().into(),
                    runtime_kind.into(),
                    status.into(),
                    source_kind.into(),
                    git("https://g").into(),
                    git("c").into(),
                    git("t").into(),
                    bundle("/u").into(),
                    bundle("h").into(),
                    with_bundle.then_some(1i64).into(),
                    key_seq.into(),
                ],
            ))
            .await
        }
    };
    let ok = |r: Result<_, sea_orm::DbErr>| r.is_ok();
    assert!(ok(insert_entry(
        "web_sandboxed",
        "required",
        "bundle",
        false,
        true,
        0
    )
    .await));
    assert!(ok(insert_entry(
        "desktop_local_process",
        "recommended",
        "git",
        true,
        false,
        0
    )
    .await));
    assert!(ok(insert_entry(
        "desktop_local_process",
        "recommended",
        "bundle",
        false,
        true,
        0
    )
    .await));
    // Unknown kind and status.
    assert!(!ok(insert_entry(
        "native",
        "recommended",
        "git",
        true,
        false,
        0
    )
    .await));
    assert!(!ok(insert_entry(
        "desktop_local_process",
        "optional",
        "git",
        true,
        false,
        0
    )
    .await));
    // Source columns that do not match the source kind.
    assert!(!ok(insert_entry(
        "desktop_local_process",
        "recommended",
        "git",
        true,
        true,
        0
    )
    .await));
    assert!(!ok(insert_entry(
        "desktop_local_process",
        "recommended",
        "bundle",
        false,
        false,
        0
    )
    .await));
    assert!(!ok(insert_entry(
        "desktop_local_process",
        "recommended",
        "zip",
        false,
        false,
        0
    )
    .await));
    // A web plugin from git, and a negative keySeq.
    assert!(!ok(insert_entry(
        "web_sandboxed",
        "recommended",
        "git",
        true,
        false,
        0
    )
    .await));
    assert!(!ok(insert_entry(
        "web_sandboxed",
        "recommended",
        "bundle",
        false,
        true,
        -1
    )
    .await));

    let insert_bundle = |sha256: String, size: i64| {
        let conn = conn.clone();
        async move {
            conn.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO plugin_bundle (server_id, sha256, size_bytes, storage_key) \
                 VALUES ($1, $2, $3, 'k')",
                [server_id.into(), sha256.into(), size.into()],
            ))
            .await
        }
    };
    assert!(ok(insert_bundle(sha(1), 1).await));
    assert!(ok(insert_bundle(sha(2), 8_388_608).await));
    assert!(!ok(insert_bundle(sha(3), 0).await));
    assert!(!ok(insert_bundle(sha(4), 8_388_609).await));
    assert!(!ok(insert_bundle(
        sha(5).to_uppercase().replace('0', "A"),
        1
    )
    .await));
    assert!(!ok(insert_bundle("abc".to_string(), 1).await));
    // One row per hash and workspace.
    assert!(!ok(insert_bundle(sha(1), 1).await));
}

/// Migration steps from the newest down to just before `add_workspace_plugin`, robust to later
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

async fn table_exists(conn: &DatabaseConnection, table: &str) -> bool {
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_regclass($1) IS NOT NULL AS present",
            [table.into()],
        ))
        .await
        .expect("query")
        .expect("row");
    row.try_get("", "present").expect("present")
}

#[tokio::test]
async fn migration_down_drops_both_tables_and_up_restores_them() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("acme").await;
    let repo = SeaOrmWorkspacePluginRepository::new(ctx.db.clone());
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, git_source()),
        100,
    )
    .await
    .unwrap();
    assert!(table_exists(&conn, "workspace_plugin").await);
    assert!(table_exists(&conn, "plugin_bundle").await);

    Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect("roll back add_workspace_plugin");
    assert!(!table_exists(&conn, "workspace_plugin").await);
    assert!(!table_exists(&conn, "plugin_bundle").await);

    Migrator::up(&conn, None).await.expect("migrate up again");
    assert!(table_exists(&conn, "workspace_plugin").await);
    assert!(table_exists(&conn, "plugin_bundle").await);
    assert!(repo.list(server.id).await.unwrap().is_empty());
    repo.insert(
        write(server.id, "dev.example.hello", owner.id, git_source()),
        100,
    )
    .await
    .unwrap()
    .expect("inserted again");
}
