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
    let ctx = support::TestCtx::new().await;

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
    let ctx = support::TestCtx::new().await;

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
async fn typed_verb_filter_returns_matching_rows_only() {
    let ctx = support::TestCtx::new().await;

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

/// All 7 `AuditOutcome` variants are accepted by the DB `outcome` CHECK constraint. A variant
/// the constraint rejected would be dropped silently via the "audit write failed" fallback.
#[tokio::test]
async fn all_audit_outcome_variants_accepted_by_db_constraint() {
    let ctx = support::TestCtx::new().await;

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

/// The `parent_audit_id` column the entity declares and the repository writes exists in the
/// schema, so an insert carrying it succeeds.
#[tokio::test]
async fn audit_insert_roundtrips_parent_audit_id() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();
    let parent_id = uuid::Uuid::now_v7();

    let mut entry = make_entry(server_id, actor_id, "test.child");
    entry.parent_audit_id = Some(parent_id);
    let entry_id = entry.id;

    repo.insert(entry)
        .await
        .expect("insert with parent_audit_id set should succeed");

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
    assert_eq!(page.items[0].parent_audit_id, Some(parent_id));
}

/// Mirrors the macro execution write order in `macro_def.rs`: a child audit
/// row (with `parent_audit_id` set) is inserted **before** the parent row
/// (with `id` equal to that same value) exists. Proves the plain nullable
/// column with no self-referencing FK tolerates child-before-parent inserts,
/// and exercises the already-widened `outcome` CHECK with a macro outcome
/// value on the child row.
#[tokio::test]
async fn audit_parent_child_linkage() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();
    let parent_id = uuid::Uuid::now_v7();

    // Child row inserted first, referencing a parent row that does not exist yet.
    let mut child = make_entry(server_id, actor_id, "test.macro_step");
    child.id = uuid::Uuid::now_v7();
    child.parent_audit_id = Some(parent_id);
    child.outcome = AuditOutcome::StepError.as_str().to_string();
    repo.insert(child)
        .await
        .expect("child insert (parent not yet written) should succeed");

    // Parent row, written after its child, per the macro_def.rs execution order.
    let mut parent = make_entry(server_id, actor_id, "test.macro");
    parent.id = parent_id;
    parent.parent_audit_id = None;
    repo.insert(parent)
        .await
        .expect("parent insert should succeed");

    // Top-level row, unrelated to the parent/child pair.
    let mut top_level = make_entry(server_id, actor_id, "test.top_level");
    top_level.parent_audit_id = None;
    repo.insert(top_level)
        .await
        .expect("top-level insert should succeed");

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

    assert_eq!(page.items.len(), 3);
    let child_row = page
        .items
        .iter()
        .find(|r| r.command_id == "test.macro_step")
        .expect("child row must be present");
    assert_eq!(child_row.parent_audit_id, Some(parent_id));
    assert_eq!(child_row.outcome, AuditOutcome::StepError.as_str());
}

#[tokio::test]
async fn retention_reads_counts_and_deletes_old_rows_in_batches() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAuditLogRepository::new(ctx.db.clone());
    let server_id = uuid::Uuid::now_v7();
    let actor_id = uuid::Uuid::now_v7();
    // Microsecond timestamps, as Postgres stores them, so keys compare exactly.
    let now = chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros())
        .expect("now");
    let cutoff = now - chrono::Duration::days(90);

    // Five old rows, two of them sharing a timestamp, and one inside the retention.
    let mut old = Vec::new();
    for days in [200, 150, 120, 120, 91] {
        let mut e = make_entry(server_id, actor_id, &format!("/old-{days}"));
        e.created_at = now - chrono::Duration::days(days);
        old.push(e);
    }
    let mut recent = make_entry(server_id, actor_id, "/recent");
    recent.created_at = now - chrono::Duration::days(1);
    for e in old.iter().chain(std::iter::once(&recent)) {
        repo.insert(e.clone()).await.expect("insert");
    }
    old.sort_by_key(|e| (e.created_at, e.id));

    let range = repo.range_older_than(cutoff).await.expect("range");
    assert_eq!(range.count, 5);
    assert_eq!(range.oldest, Some(old[0].created_at));
    assert_eq!(range.newest, Some(old[4].created_at));
    let empty = repo
        .range_older_than(now - chrono::Duration::days(365))
        .await
        .expect("range");
    assert_eq!((empty.count, empty.oldest, empty.newest), (0, None, None));

    // Keyset pages walk the old rows oldest first without skipping the timestamp tie.
    let page1 = repo.list_older_than(cutoff, None, 2).await.expect("page 1");
    let last = page1.last().expect("rows");
    let page2 = repo
        .list_older_than(cutoff, Some((last.created_at, last.id)), 2)
        .await
        .expect("page 2");
    let last = page2.last().expect("rows");
    let page3 = repo
        .list_older_than(cutoff, Some((last.created_at, last.id)), 2)
        .await
        .expect("page 3");
    let walked: Vec<_> = page1
        .iter()
        .chain(&page2)
        .chain(&page3)
        .map(|e| e.id)
        .collect();
    assert_eq!(walked, old.iter().map(|e| e.id).collect::<Vec<_>>());

    // Deletes stop at the bound: the newest old row stays until the bound covers it.
    let bound = (old[3].created_at, old[3].id);
    assert_eq!(
        repo.delete_older_than_batch(cutoff, bound, 3)
            .await
            .expect("batch 1"),
        3
    );
    assert_eq!(
        repo.delete_older_than_batch(cutoff, bound, 3)
            .await
            .expect("batch 2"),
        1
    );
    assert_eq!(
        repo.delete_older_than_batch(cutoff, bound, 3)
            .await
            .expect("batch 3"),
        0
    );
    let left = repo.list_older_than(cutoff, None, 10).await.expect("left");
    assert_eq!(
        left.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![old[4].id]
    );
    let all = repo
        .list_older_than(now + chrono::Duration::days(1), None, 10)
        .await
        .expect("all");
    assert_eq!(
        all.len(),
        2,
        "the old row past the bound and the recent row"
    );
}
