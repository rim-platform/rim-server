#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::incident::model::{IncidentSeverity, IncidentStatus, NewIncident};
use rim_core::incident::repository::IncidentRepository;
use rim_core::repository::pagination::PageDir;
use rim_infra_db::sea_orm_impl::incident::SeaOrmIncidentRepository;

#[tokio::test]
async fn list_incidents_orders_desc_and_paginates() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;

    // Three incidents created in order; expect listing returns most-recent-first. Each gets its
    // own conversation — `idx_incident_active_conversation` allows at most one *active* incident
    // per conversation, and all three are created active.
    let mut created = Vec::new();
    for n in 0..3 {
        let conv = ctx
            .create_conversation(server.id, ConversationType::Channel)
            .await;
        let inc = repo
            .create(NewIncident {
                server_id: server.id,
                conversation_id: conv.id,
                started_by: user.id,
                severity: Some(IncidentSeverity::Sev2),
                title: format!("incident-{n}"),
                original_topic: None,
                alert_fingerprint: None,
                source: "manual".to_string(),
                escalation_policy_id: None,
            })
            .await
            .unwrap();
        created.push(inc);
    }

    // Page 1: limit=2, no cursor → newest two (rev order of `created`).
    let page1 = repo
        .list_incidents(server.id, None, None, 2, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.items[0].id, created[2].id);
    assert_eq!(page1.items[1].id, created[1].id);
    assert!(page1.next.is_some(), "expected next cursor (more rows)");
    assert!(page1.prev.is_none(), "no opposite when no input cursor");

    // Page 2: continue with returned cursor in same direction.
    let page2 = repo
        .list_incidents(server.id, None, None, 2, PageDir::After, page1.next)
        .await
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.items[0].id, created[0].id);
    assert!(page2.next.is_none(), "no more pages after");
    assert!(
        page2.prev.is_some(),
        "prev cursor available since cursor was set"
    );
}

#[tokio::test]
async fn list_incidents_filters_status_and_severity() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;

    // Each incident gets its own conversation — `idx_incident_active_conversation` allows at
    // most one *active* incident per conversation, and all three are created active (even
    // `mitigated_sev2`, which is transitioned to Mitigated only after creation below).
    let conv1 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let conv2 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let conv3 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let active_sev1 = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv1.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev1),
            title: "active-sev1".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();

    let active_sev3 = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv2.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev3),
            title: "active-sev3".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();

    let mitigated_sev2 = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv3.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "mitigated-sev2".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();
    repo.update_status(
        mitigated_sev2.id,
        IncidentStatus::Mitigated,
        Some(chrono::Utc::now()),
        None,
    )
    .await
    .unwrap();

    // Filter status=Active → both sev1 + sev3.
    let only_active = repo
        .list_incidents(
            server.id,
            Some(&[IncidentStatus::Active]),
            None,
            10,
            PageDir::After,
            None,
        )
        .await
        .unwrap();
    let active_ids: Vec<_> = only_active.items.iter().map(|i| i.id).collect();
    assert_eq!(active_ids.len(), 2);
    assert!(active_ids.contains(&active_sev1.id));
    assert!(active_ids.contains(&active_sev3.id));

    // Filter severity in [Sev1, Sev2] → active_sev1 + mitigated_sev2.
    let by_severity = repo
        .list_incidents(
            server.id,
            None,
            Some(&[IncidentSeverity::Sev1, IncidentSeverity::Sev2]),
            10,
            PageDir::After,
            None,
        )
        .await
        .unwrap();
    let sev_ids: Vec<_> = by_severity.items.iter().map(|i| i.id).collect();
    assert_eq!(sev_ids.len(), 2);
    assert!(sev_ids.contains(&active_sev1.id));
    assert!(sev_ids.contains(&mitigated_sev2.id));

    // Combined AND: status=Active AND severity in [Sev1, Sev2] → only active_sev1.
    let both = repo
        .list_incidents(
            server.id,
            Some(&[IncidentStatus::Active]),
            Some(&[IncidentSeverity::Sev1, IncidentSeverity::Sev2]),
            10,
            PageDir::After,
            None,
        )
        .await
        .unwrap();
    assert_eq!(both.items.len(), 1);
    assert_eq!(both.items[0].id, active_sev1.id);

    // Empty filter array behaves the same as None (no filter on that field).
    let empty_status = repo
        .list_incidents(server.id, Some(&[]), None, 10, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(empty_status.items.len(), 3);
}

#[tokio::test]
async fn list_incidents_scopes_by_server() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server_a = ctx.create_server("a").await;
    let server_b = ctx.create_server("b").await;
    let user = ctx.create_user("u").await;

    let conv_a = ctx
        .create_conversation(server_a.id, ConversationType::Channel)
        .await;
    let conv_b = ctx
        .create_conversation(server_b.id, ConversationType::Channel)
        .await;

    let in_a = repo
        .create(NewIncident {
            server_id: server_a.id,
            conversation_id: conv_a.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "in-a".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();
    let _in_b = repo
        .create(NewIncident {
            server_id: server_b.id,
            conversation_id: conv_b.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "in-b".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();

    let only_a = repo
        .list_incidents(server_a.id, None, None, 10, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(only_a.items.len(), 1);
    assert_eq!(only_a.items[0].id, in_a.id);
}

// ── Track B2: alert_fingerprint / source ────────────────────────────────────

#[tokio::test]
async fn create_persists_alert_fingerprint_and_source() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let manual = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "manual".into(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();
    assert_eq!(manual.alert_fingerprint, None);
    assert_eq!(manual.source, "manual");

    let conv2 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let alerted = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv2.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev1),
            title: "alerted".into(),
            original_topic: None,
            alert_fingerprint: Some("fp-persist".to_string()),
            source: "alertmanager".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();
    assert_eq!(alerted.alert_fingerprint.as_deref(), Some("fp-persist"));
    assert_eq!(alerted.source, "alertmanager");

    let fetched = repo.find_by_id(alerted.id).await.unwrap().unwrap();
    assert_eq!(fetched.alert_fingerprint.as_deref(), Some("fp-persist"));
    assert_eq!(fetched.source, "alertmanager");
}

#[tokio::test]
async fn find_by_fingerprint_matches_only_non_resolved() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let incident = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "firing".into(),
            original_topic: None,
            alert_fingerprint: Some("fp-active".to_string()),
            source: "generic".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();

    let found = repo
        .find_by_fingerprint(server.id, "fp-active")
        .await
        .unwrap();
    assert_eq!(found.map(|i| i.id), Some(incident.id));

    // Once resolved, the same fingerprint no longer matches — a later alert with the same
    // fingerprint should create a fresh incident rather than reopening this one.
    repo.update_status(
        incident.id,
        IncidentStatus::Resolved,
        None,
        Some(chrono::Utc::now()),
    )
    .await
    .unwrap();

    let after_resolve = repo
        .find_by_fingerprint(server.id, "fp-active")
        .await
        .unwrap();
    assert!(after_resolve.is_none());
}

#[tokio::test]
async fn find_by_fingerprint_scopes_by_server() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server_a = ctx.create_server("a").await;
    let server_b = ctx.create_server("b").await;
    let user = ctx.create_user("u").await;
    let conv_a = ctx
        .create_conversation(server_a.id, ConversationType::Channel)
        .await;

    let in_a = repo
        .create(NewIncident {
            server_id: server_a.id,
            conversation_id: conv_a.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "shared-fp".into(),
            original_topic: None,
            alert_fingerprint: Some("shared-fp".to_string()),
            source: "generic".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap();

    // Same fingerprint string, but scoped to a different server — must not match.
    let found_in_b = repo
        .find_by_fingerprint(server_b.id, "shared-fp")
        .await
        .unwrap();
    assert!(found_in_b.is_none());

    let found_in_a = repo
        .find_by_fingerprint(server_a.id, "shared-fp")
        .await
        .unwrap();
    assert_eq!(found_in_a.map(|i| i.id), Some(in_a.id));
}

/// `idx_incident_active_fingerprint` — at most one non-resolved incident per
/// (server, fingerprint), enforced at the DB level. Uses two *different* conversations so this
/// doesn't collide with the unrelated `idx_incident_active_conversation` index.
#[tokio::test]
async fn duplicate_active_fingerprint_on_same_server_violates_unique_index() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIncidentRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv1 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let conv2 = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    repo.create(NewIncident {
        server_id: server.id,
        conversation_id: conv1.id,
        started_by: user.id,
        severity: Some(IncidentSeverity::Sev2),
        title: "first".into(),
        original_topic: None,
        alert_fingerprint: Some("fp-dup".to_string()),
        source: "generic".to_string(),
        escalation_policy_id: None,
    })
    .await
    .unwrap();

    let err = repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv2.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "second".into(),
            original_topic: None,
            alert_fingerprint: Some("fp-dup".to_string()),
            source: "generic".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    );
}
