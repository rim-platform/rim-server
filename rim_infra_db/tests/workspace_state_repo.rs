//! Workspace state writes over an entry that expired but is not purged yet: the write replaces
//! it as a new entry of its writer, while a write over a live entry keeps its creator.
#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};
use serde_json::json;

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_state::model::{NewWorkspaceState, WorkspaceState, WorkspaceStateScope};
use rim_core::workspace_state::repository::WorkspaceStateRepository;
use rim_infra_db::sea_orm_impl::workspace_state::SeaOrmWorkspaceStateRepository;

fn entry(
    server_id: ServerId,
    created_by: UserId,
    value: serde_json::Value,
    ttl: Option<Duration>,
) -> NewWorkspaceState {
    NewWorkspaceState {
        server_id,
        key: "board.temp".to_string(),
        value,
        scope: WorkspaceStateScope::Server,
        user_id: None,
        created_by,
        expires_at: ttl.map(|ttl| Utc::now() + ttl),
    }
}

async fn stored(repo: &SeaOrmWorkspaceStateRepository, server_id: ServerId) -> WorkspaceState {
    repo.get_by_key(server_id, WorkspaceStateScope::Server, None, "board.temp")
        .await
        .unwrap()
        .expect("live entry")
}

#[tokio::test]
async fn upsert_over_an_expired_entry_makes_it_the_writers() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceStateRepository::new(ctx.db.clone());
    let server = ctx.create_server("state").await;
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;

    let first = repo
        .upsert(entry(
            server.id,
            ada.id,
            json!("ada"),
            Some(Duration::hours(1)),
        ))
        .await
        .unwrap();
    let live = repo
        .upsert(entry(
            server.id,
            bob.id,
            json!("bob"),
            Some(Duration::seconds(-1)),
        ))
        .await
        .unwrap();
    assert_eq!(
        (live.id, live.created_by, live.updated_by, live.version),
        (first.id, ada.id, bob.id, 2),
        "a live entry keeps its creator"
    );
    assert_eq!(live.created_at, first.created_at);

    let taken = repo
        .upsert(entry(server.id, bob.id, json!("bob"), None))
        .await
        .unwrap();
    assert_eq!(
        (
            taken.created_by,
            taken.updated_by,
            taken.version,
            taken.expires_at
        ),
        (bob.id, bob.id, 1, None)
    );
    assert!(taken.created_at > first.created_at);
    assert_eq!(stored(&repo, server.id).await.created_by, bob.id);

    let usage = |user| repo.creator_usage(server.id, user);
    assert_eq!(usage(ada.id).await.unwrap().entries, 0);
    assert_eq!(usage(bob.id).await.unwrap().entries, 1);
}

#[tokio::test]
async fn increment_over_an_expired_entry_starts_from_the_delta() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceStateRepository::new(ctx.db.clone());
    let server = ctx.create_server("state").await;
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;
    let increment = |actor| {
        repo.increment_by_key(
            server.id,
            WorkspaceStateScope::Server,
            None,
            "board.temp",
            2,
            actor,
        )
    };

    repo.upsert(entry(
        server.id,
        ada.id,
        json!(40),
        Some(Duration::hours(1)),
    ))
    .await
    .unwrap();
    let live = increment(bob.id).await.unwrap();
    assert_eq!(
        (live.value.clone(), live.created_by, live.version),
        (json!(42), ada.id, 2),
        "a live entry adds the delta and keeps its creator"
    );
    assert!(live.expires_at.is_some());

    // A stale value, even a non-numeric one, is replaced rather than added to.
    repo.upsert(entry(
        server.id,
        ada.id,
        json!("text"),
        Some(Duration::seconds(-1)),
    ))
    .await
    .unwrap();
    let taken = increment(bob.id).await.unwrap();
    assert_eq!(
        (
            taken.value.clone(),
            taken.created_by,
            taken.version,
            taken.expires_at
        ),
        (json!(2), bob.id, 1, None)
    );
    assert_eq!(stored(&repo, server.id).await.value, json!(2));
}
