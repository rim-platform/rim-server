#![cfg(feature = "db-tests")]

//! Public join links, their join records and join requests against a live Postgres.
//! Destructive for the shared test database: run single-threaded.

mod support;

use chrono::{Duration, SubsecRound, Utc};

use rim_core::public_join_link::model::{
    JoinRequestStatus, NewJoinRequest, PublicJoinLink, PublicLinkMode,
};
use rim_core::public_join_link::repository::{ListJoinRequestsParams, PublicJoinLinkRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::public_join_link::SeaOrmPublicJoinLinkRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;

fn link(server_id: ServerId, hash: &str, issued_by: Option<UserId>) -> PublicJoinLink {
    PublicJoinLink {
        server_id,
        token_hash: hash.to_string(),
        sealed_token: vec![1, 2, 3],
        enabled: true,
        mode: PublicLinkMode::Requests,
        role: ServerMemberRole::Guest,
        allow_remote: false,
        enabled_at: Some(Utc::now().trunc_subsecs(6)),
        issued_at: Utc::now().trunc_subsecs(6),
        issued_by,
    }
}

fn new_request(server_id: ServerId, user_id: UserId, at: chrono::DateTime<Utc>) -> NewJoinRequest {
    NewJoinRequest {
        server_id,
        user_id,
        message: Some("hello".to_string()),
        created_at: at,
        expires_at: at + Duration::days(30),
        relay_server_id: None,
    }
}

#[tokio::test]
async fn a_link_is_saved_replaced_found_by_hash_and_goes_with_its_workspace() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPublicJoinLinkRepository::new(ctx.db.clone());
    let server = ctx.create_server("lobby").await;
    let owner = ctx.create_user("owner").await;

    assert_eq!(repo.link(server.id).await.unwrap(), None);
    assert_eq!(repo.count_links().await.unwrap(), 0);
    let first = link(server.id, "hash-1", Some(owner.id));
    assert_eq!(repo.save_link(first.clone()).await.unwrap(), first);
    assert_eq!(repo.link(server.id).await.unwrap(), Some(first.clone()));

    let second = PublicJoinLink {
        token_hash: "hash-2".to_string(),
        enabled: false,
        mode: PublicLinkMode::Direct,
        role: ServerMemberRole::Member,
        allow_remote: true,
        enabled_at: None,
        issued_by: None,
        ..first.clone()
    };
    repo.save_link(second.clone()).await.unwrap();
    assert_eq!(repo.link_by_token_hash("hash-1").await.unwrap(), None);
    assert_eq!(
        repo.link_by_token_hash("hash-2").await.unwrap(),
        Some(second)
    );
    assert_eq!(repo.count_links().await.unwrap(), 1);

    SeaOrmServerRepository::new(ctx.db.clone())
        .delete_workspaces(&[server.id])
        .await
        .unwrap();
    assert_eq!(repo.link_by_token_hash("hash-2").await.unwrap(), None);
    assert_eq!(repo.count_links().await.unwrap(), 0);
}

#[tokio::test]
async fn joins_are_counted_by_time_and_records_older_than_a_week_are_dropped() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPublicJoinLinkRepository::new(ctx.db.clone());
    let server = ctx.create_server("lobby").await;
    let other = ctx.create_server("other").await;
    let (alice, bob) = (ctx.create_user("alice").await, ctx.create_user("bob").await);
    let now = Utc::now();

    repo.record_join(server.id, alice.id, now - Duration::days(8))
        .await
        .unwrap();
    repo.record_join(server.id, bob.id, now - Duration::days(2))
        .await
        .unwrap();
    repo.record_join(other.id, bob.id, now).await.unwrap();
    let month = now - Duration::days(30);
    assert_eq!(repo.count_joins_since(server.id, month).await.unwrap(), 2);
    repo.record_join(server.id, alice.id, now).await.unwrap();
    assert_eq!(
        repo.count_joins_since(server.id, month).await.unwrap(),
        2,
        "writing a record at `now` dropped the one from eight days before"
    );
    assert_eq!(
        repo.count_joins_since(server.id, now - Duration::hours(24))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_joins_since(server.id, now - Duration::days(7))
            .await
            .unwrap(),
        2
    );
    assert_eq!(repo.count_joins_since(other.id, month).await.unwrap(), 1);
}

#[tokio::test]
async fn one_open_request_per_person_and_workspace_until_it_closes_or_expires() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPublicJoinLinkRepository::new(ctx.db.clone());
    let server = ctx.create_server("lobby").await;
    let alice = ctx.create_user("alice").await;
    let moderator = ctx.create_user("moderator").await;
    let now = Utc::now().trunc_subsecs(6);

    let request = repo
        .create_request(new_request(server.id, alice.id, now))
        .await
        .unwrap();
    assert_eq!(request.status, JoinRequestStatus::Open);
    assert_eq!(request.message.as_deref(), Some("hello"));
    assert_eq!(
        repo.request(request.id).await.unwrap(),
        Some(request.clone())
    );
    let again = repo
        .create_request(new_request(server.id, alice.id, now))
        .await
        .unwrap_err();
    assert_eq!(again.kind(), RepoErrorKind::Conflict);
    assert_eq!(
        repo.open_request(server.id, alice.id, now).await.unwrap(),
        Some(request.clone())
    );
    assert_eq!(repo.count_open_for_user(alice.id, now).await.unwrap(), 1);
    assert_eq!(repo.count_open_for_server(server.id, now).await.unwrap(), 1);

    // Past its expiry it no longer counts, and a new request marks it expired.
    let later = now + Duration::days(30);
    assert_eq!(
        repo.open_request(server.id, alice.id, later).await.unwrap(),
        None
    );
    assert_eq!(repo.count_open_for_user(alice.id, later).await.unwrap(), 0);
    assert_eq!(
        repo.close_request(
            request.id,
            JoinRequestStatus::Declined,
            Some(moderator.id),
            later
        )
        .await
        .unwrap(),
        None
    );
    let fresh = repo
        .create_request(new_request(server.id, alice.id, later))
        .await
        .unwrap();
    assert_eq!(
        repo.request(request.id).await.unwrap().unwrap().status,
        JoinRequestStatus::Expired
    );

    let declined = repo
        .close_request(
            fresh.id,
            JoinRequestStatus::Declined,
            Some(moderator.id),
            later,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(declined.status, JoinRequestStatus::Declined);
    assert_eq!(declined.decided_by, Some(moderator.id));
    assert_eq!(declined.decided_at, Some(later));
    assert_eq!(
        repo.close_request(
            fresh.id,
            JoinRequestStatus::Accepted,
            Some(moderator.id),
            later
        )
        .await
        .unwrap(),
        None,
        "a closed request stays closed"
    );
    assert_eq!(
        repo.last_declined_at(server.id, alice.id).await.unwrap(),
        Some(later)
    );
    assert_eq!(
        repo.last_declined_at(server.id, moderator.id)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn open_requests_page_newest_first_and_a_ban_cancels_them() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPublicJoinLinkRepository::new(ctx.db.clone());
    let server = ctx.create_server("lobby").await;
    let other = ctx.create_server("other").await;
    let now = Utc::now().trunc_subsecs(6);
    let mut people = Vec::new();
    for i in 0..5 {
        let user = ctx.create_user(&format!("user{i}")).await;
        repo.create_request(new_request(server.id, user.id, now + Duration::seconds(i)))
            .await
            .unwrap();
        people.push(user.id);
    }
    repo.create_request(new_request(other.id, people[0], now))
        .await
        .unwrap();

    let params = |dir, cursor| ListJoinRequestsParams {
        server_id: server.id,
        now,
        limit: 2,
        dir,
        cursor,
    };
    let first = repo
        .list_open_requests(params(PageDir::After, None))
        .await
        .unwrap();
    let ids = |page: &rim_core::repository::pagination::Page<_, _>| -> Vec<UserId> {
        page.items
            .iter()
            .map(|r: &rim_core::public_join_link::model::JoinRequest| r.user_id)
            .collect()
    };
    assert_eq!(ids(&first), vec![people[4], people[3]]);
    assert!(first.prev.is_none());
    let second = repo
        .list_open_requests(params(PageDir::After, first.next))
        .await
        .unwrap();
    assert_eq!(ids(&second), vec![people[2], people[1]]);
    let third = repo
        .list_open_requests(params(PageDir::After, second.next))
        .await
        .unwrap();
    assert_eq!(ids(&third), vec![people[0]]);
    assert!(third.next.is_none());
    let back = repo
        .list_open_requests(params(PageDir::Before, third.prev))
        .await
        .unwrap();
    assert_eq!(ids(&back), vec![people[2], people[1]]);

    let mine = repo.list_open_for_user(people[0], now).await.unwrap();
    assert_eq!(mine.len(), 2);

    let cancelled = repo
        .cancel_open_requests(server.id, people[0], now)
        .await
        .unwrap();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].status, JoinRequestStatus::Cancelled);
    assert_eq!(repo.count_open_for_server(server.id, now).await.unwrap(), 4);
    assert_eq!(repo.count_open_for_user(people[0], now).await.unwrap(), 1);
}
