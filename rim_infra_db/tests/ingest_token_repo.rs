#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::incident::model::IncidentSeverity;
use rim_core::ingest_token::model::NewIngestToken;
use rim_core::ingest_token::repository::IngestTokenRepository;
use rim_infra_db::sea_orm_impl::ingest_token::SeaOrmIngestTokenRepository;

/// A repo test verifies persistence and lookup, not the hash function itself (that's covered
/// by `rim_auth`'s own tests and `rim_infra_services::ingest_token`'s `authenticate_token`
/// tests) — a fixed hash-shaped literal is enough here.
#[tokio::test]
async fn create_persists_hash_and_roundtrips_by_id_and_hash() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let hash = "a".repeat(64);

    let created = repo
        .create(NewIngestToken {
            server_id: server.id,
            conversation_id: conv.id,
            created_by: user.id,
            label: "Prometheus prod".to_string(),
            token_hash: hash.clone(),
            severity_default: Some(IncidentSeverity::Sev2),
        })
        .await
        .unwrap();

    assert_eq!(created.token_hash, hash);
    assert_eq!(created.label, "Prometheus prod");
    assert_eq!(created.severity_default, Some(IncidentSeverity::Sev2));
    assert!(created.revoked_at.is_none());
    assert!(created.last_used_at.is_none());

    // find_by_id roundtrip.
    let fetched = repo.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);

    // find_by_token_hash roundtrip — this is the webhook auth lookup path.
    let by_hash = repo.find_by_token_hash(&hash).await.unwrap().unwrap();
    assert_eq!(by_hash.id, created.id);
}

#[tokio::test]
async fn token_hash_is_unique() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let hash = "b".repeat(64);

    repo.create(NewIngestToken {
        server_id: server.id,
        conversation_id: conv.id,
        created_by: user.id,
        label: "first".to_string(),
        token_hash: hash.clone(),
        severity_default: None,
    })
    .await
    .unwrap();

    let err = repo
        .create(NewIngestToken {
            server_id: server.id,
            conversation_id: conv.id,
            created_by: user.id,
            label: "second".to_string(),
            token_hash: hash,
            severity_default: None,
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    );
}

#[tokio::test]
async fn find_by_token_hash_returns_none_for_unknown_hash() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let none = repo
        .find_by_token_hash("0000000000000000000000000000000000000000000000000000000000000000")
        .await
        .unwrap();
    assert!(none.is_none());
}

#[tokio::test]
async fn revoke_sets_revoked_at_and_is_idempotent_on_read() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let created = repo
        .create(NewIngestToken {
            server_id: server.id,
            conversation_id: conv.id,
            created_by: user.id,
            label: "to-revoke".to_string(),
            token_hash: "c".repeat(64),
            severity_default: None,
        })
        .await
        .unwrap();
    assert!(created.revoked_at.is_none());

    repo.revoke(created.id).await.unwrap();

    let fetched = repo.find_by_id(created.id).await.unwrap().unwrap();
    assert!(fetched.revoked_at.is_some());
}

#[tokio::test]
async fn touch_last_used_updates_timestamp() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let created = repo
        .create(NewIngestToken {
            server_id: server.id,
            conversation_id: conv.id,
            created_by: user.id,
            label: "touch".to_string(),
            token_hash: "d".repeat(64),
            severity_default: None,
        })
        .await
        .unwrap();
    assert!(created.last_used_at.is_none());

    let now = chrono::Utc::now();
    repo.touch_last_used(created.id, now).await.unwrap();

    let fetched = repo.find_by_id(created.id).await.unwrap().unwrap();
    let last_used = fetched.last_used_at.unwrap();
    assert!((last_used - now).num_seconds().abs() < 2);
}

#[tokio::test]
async fn list_for_server_scopes_and_orders_newest_first() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIngestTokenRepository::new(ctx.db.clone());

    let server_a = ctx.create_server("a").await;
    let server_b = ctx.create_server("b").await;
    let user = ctx.create_user("u").await;
    let conv_a = ctx
        .create_conversation(server_a.id, ConversationType::Channel)
        .await;
    let conv_b = ctx
        .create_conversation(server_b.id, ConversationType::Channel)
        .await;

    let first = repo
        .create(NewIngestToken {
            server_id: server_a.id,
            conversation_id: conv_a.id,
            created_by: user.id,
            label: "first".to_string(),
            token_hash: "e".repeat(64),
            severity_default: None,
        })
        .await
        .unwrap();
    let second = repo
        .create(NewIngestToken {
            server_id: server_a.id,
            conversation_id: conv_a.id,
            created_by: user.id,
            label: "second".to_string(),
            token_hash: "f".repeat(64),
            severity_default: None,
        })
        .await
        .unwrap();
    let _in_b = repo
        .create(NewIngestToken {
            server_id: server_b.id,
            conversation_id: conv_b.id,
            created_by: user.id,
            label: "in-b".to_string(),
            token_hash: "0".repeat(64),
            severity_default: None,
        })
        .await
        .unwrap();

    let listed = repo.list_for_server(server_a.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    // Newest first.
    assert_eq!(listed[0].id, second.id);
    assert_eq!(listed[1].id, first.id);
}
