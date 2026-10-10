//! The audit retention purge against a live database: old rows are exported to a gzipped JSON
//! Lines file and then deleted, rows inside the retention stay.
//!
//! Only compiled with `--features infra-db-tests`; needs `DATABASE_URL` (see `support`).

#![cfg(feature = "infra-db-tests")]

mod support;

use std::fs::File;
use std::io::{BufRead, BufReader};

use chrono::{DateTime, Duration, Utc};
use flate2::read::GzDecoder;
use rim_core::audit::repository::{AuditLogEntryRecord, AuditLogListQuery, AuditLogRepository};
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::audit::SeaOrmAuditLogRepository;
use rim_infra_services::audit_retention;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

fn entry(server_id: Uuid, command_id: &str, created_at: DateTime<Utc>) -> AuditLogEntryRecord {
    AuditLogEntryRecord {
        id: Uuid::now_v7(),
        actor_user_id: Uuid::now_v7(),
        server_id,
        command_id: command_id.to_string(),
        initiator: "palette".to_string(),
        args_redacted: serde_json::json!({"channel": "[redacted]"}),
        sensitivity: "low".to_string(),
        dangerous: false,
        confirmation_kind: "none".to_string(),
        typed_verb: Some("write".to_string()),
        outcome: "success".to_string(),
        error_kind: None,
        capability_check_outcome: None,
        confirmation_outcome: None,
        parent_audit_id: None,
        created_at,
    }
}

#[tokio::test]
async fn one_purge_exports_the_old_rows_and_keeps_the_recent_ones() {
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    db.execute_unprepared("TRUNCATE TABLE command_audit_log")
        .await
        .expect("truncate");
    let repo = SeaOrmAuditLogRepository::new(DbHandle::Connection(db.clone()));
    let server_id = Uuid::now_v7();
    let now = Utc::now();

    // More old rows than one batch, so the export pages and the delete loops.
    let old_count = audit_retention::BATCH_SIZE as usize + 7;
    for i in 0..old_count {
        let at = now - Duration::days(120) + Duration::seconds(i as i64);
        repo.insert(entry(server_id, &format!("/old-{i}"), at))
            .await
            .expect("insert old");
    }
    for i in 0..3 {
        repo.insert(entry(
            server_id,
            &format!("/recent-{i}"),
            now - Duration::days(i),
        ))
        .await
        .expect("insert recent");
    }

    let data_dir = tempfile::tempdir().expect("tempdir");
    let export_dir = audit_retention::export_dir(data_dir.path());
    let cutoff = audit_retention::cutoff(now, 90);
    let report = audit_retention::run(&repo, cutoff, &export_dir, "job")
        .await
        .expect("purge");

    assert_eq!(report.exported, old_count as u64);
    assert_eq!(report.deleted, old_count as u64);
    let path = report.export_path.expect("an export file");
    assert_eq!(path.parent(), Some(export_dir.as_path()));
    let name = path
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("audit-") && name.ends_with(".jsonl.gz"),
        "{name}"
    );

    let lines: Vec<serde_json::Value> =
        BufReader::new(GzDecoder::new(File::open(&path).expect("open export")))
            .lines()
            .map(|l| serde_json::from_str(&l.expect("line")).expect("json"))
            .collect();
    assert_eq!(lines.len(), old_count);
    assert_eq!(lines[0]["commandId"], "/old-0");
    assert_eq!(
        lines[old_count - 1]["commandId"],
        format!("/old-{}", old_count - 1)
    );
    assert_eq!(lines[0]["serverId"], server_id.to_string());
    assert_eq!(lines[0]["argsRedacted"]["channel"], "[redacted]");

    let remaining = repo
        .list(AuditLogListQuery {
            actor_user_id: None,
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 200,
        })
        .await
        .expect("list");
    let mut commands: Vec<_> = remaining
        .items
        .iter()
        .map(|e| e.command_id.clone())
        .collect();
    commands.sort();
    assert_eq!(commands, vec!["/recent-0", "/recent-1", "/recent-2"]);

    // A second run finds nothing and writes no further file.
    let again = audit_retention::run(&repo, cutoff, &export_dir, "job")
        .await
        .expect("second purge");
    assert_eq!((again.exported, again.deleted), (0, 0));
    assert_eq!(std::fs::read_dir(&export_dir).expect("dir").count(), 1);
}
