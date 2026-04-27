#![cfg(feature = "db-tests")]

mod support;

use rim_core::audit::repository::{AuditLogEntryRecord, AuditLogListQuery, AuditLogRepository};
use rim_infra_db::sea_orm_impl::audit::SeaOrmAuditLogRepository;
use rim_services::audit::dto::AuditOutcome;

fn make_entry(
    server_id: uuid::Uuid,
    actor_id: uuid::Uuid,
    command_id: &str,
) -> AuditLogEntryRecord {
    AuditLogEntryRecord {
        id: uuid::Uuid::now_v7(),
        actor_user_id: actor_id,
        server_id,
        command_id: command_id.to_string(),
        initiator: "palette".to_string(),
        args_redacted: serde_json::json!({}),
        sensitivity: "low".to_string(),
        dangerous: false,
        confirmation_kind: "none".to_string(),
        typed_verb: None,
        outcome: "success".to_string(),
        error_kind: None,
        capability_check_outcome: None,
        confirmation_outcome: None,
        parent_audit_id: None,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn insert_and_list_roundtrip() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();

    let entry = make_entry(server_id, actor_id, "/ping");
    let entry_id = entry.id;

    repo.insert(entry).await.expect("insert should succeed");

    let page = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list should succeed");

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, entry_id);
    assert_eq!(page.items[0].command_id, "/ping");
    assert!(!page.has_next_page);
    assert!(!page.has_prev_page);
}

#[tokio::test]
async fn pagination_cursor_advances_pages() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();

    // Insert 5 entries with slight timestamp differences (UUIDv7 ensures ordering).
    for i in 0..5u32 {
        let mut e = make_entry(server_id, actor_id, &format!("/cmd-{i}"));
        // Offset created_at so ordering is deterministic.
        e.created_at = chrono::Utc::now() - chrono::Duration::seconds(i as i64);
        repo.insert(e).await.expect("insert should succeed");
    }

    // Page 1: limit=2
    let page1 = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 2,
        })
        .await
        .expect("list page 1");
    assert_eq!(page1.items.len(), 2);
    assert!(page1.has_next_page);
    assert!(!page1.has_prev_page);
    assert!(page1.next_cursor.is_some());

    // Page 2: limit=2, after page1 cursor
    let page2 = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: page1.next_cursor,
            limit: 2,
        })
        .await
        .expect("list page 2");
    assert_eq!(page2.items.len(), 2);
    assert!(page2.has_next_page);
    assert!(page2.has_prev_page);

    // Page 3: last page
    let page3 = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: page2.next_cursor,
            limit: 2,
        })
        .await
        .expect("list page 3");
    assert_eq!(page3.items.len(), 1);
    assert!(!page3.has_next_page);
    assert!(page3.has_prev_page);
}

#[tokio::test]
async fn delete_older_than_removes_stale_rows() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();

    // Old entry: 100 days ago.
    let mut old = make_entry(server_id, actor_id, "/old");
    old.created_at = chrono::Utc::now() - chrono::Duration::days(100);
    repo.insert(old).await.expect("insert old");

    // Recent entry: 1 day ago.
    let mut recent = make_entry(server_id, actor_id, "/recent");
    recent.created_at = chrono::Utc::now() - chrono::Duration::days(1);
    repo.insert(recent).await.expect("insert recent");

    // Delete rows older than 90 days.
    let cutoff = chrono::Utc::now() - chrono::Duration::days(90);
    let deleted = repo
        .delete_older_than(cutoff)
        .await
        .expect("delete_older_than");
    assert_eq!(deleted, 1, "only the old row should be deleted");

    // Verify only the recent entry remains.
    let page = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list after deletion");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].command_id, "/recent");
}

#[tokio::test]
async fn typed_verb_filter_returns_matching_rows_only() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();

    // Insert: NULL verb, "write" verb, "read" verb.
    let mut null_verb = make_entry(server_id, actor_id, "/cmd-null");
    null_verb.typed_verb = None;
    repo.insert(null_verb).await.expect("insert null verb");

    let mut write_verb = make_entry(server_id, actor_id, "/cmd-write");
    write_verb.typed_verb = Some("write".to_string());
    repo.insert(write_verb).await.expect("insert write verb");

    let mut read_verb = make_entry(server_id, actor_id, "/cmd-read");
    read_verb.typed_verb = Some("read".to_string());
    repo.insert(read_verb).await.expect("insert read verb");

    // Filter = Some("write") → 1 row.
    let filtered = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: Some("write".to_string()),
            since: None,
            until: None,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list filtered");
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].command_id, "/cmd-write");

    // No filter → 3 rows.
    let unfiltered = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list unfiltered");
    assert_eq!(unfiltered.items.len(), 3);
}

/// P6-M1 regression: all 7 `AuditOutcome` variants must be accepted by the
/// DB `outcome` CHECK constraint after migration `m20260427_000001_audit_outcome_widen_check`.
///
/// Before the fix, `rate_limited`, `step_error`, and `macro_partial_failure`
/// violated the 4-value CHECK in the original migration and were silently
/// dropped via the "audit write failed" fallback.
#[tokio::test]
async fn all_audit_outcome_variants_accepted_by_db_constraint() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();

    for outcome in AuditOutcome::all() {
        let mut entry = make_entry(server_id, actor_id, "test.cmd");
        entry.outcome = outcome.as_str().to_string();
        repo.insert(entry).await.unwrap_or_else(|e| {
            panic!(
                "insert with outcome {:?} ({}) failed: {e}",
                outcome,
                outcome.as_str()
            )
        });
    }

    // All 7 rows must be retrievable.
    let page = repo
        .list(AuditLogListQuery {
            actor_user_id: Some(actor_id),
            server_id: Some(server_id),
            command_id: None,
            typed_verb: None,
            since: None,
            until: None,
            cursor: None,
            limit: 20,
        })
        .await
        .expect("list all outcome rows");

    assert_eq!(
        page.items.len(),
        AuditOutcome::all().len(),
        "expected {} rows (one per outcome variant), got {}",
        AuditOutcome::all().len(),
        page.items.len()
    );
}
