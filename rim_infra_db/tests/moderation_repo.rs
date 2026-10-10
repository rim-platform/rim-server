#![cfg(feature = "db-tests")]

//! Bans, timeouts, reports, slow mode and the role migration data step against a
//! live Postgres. Destructive for the shared test database: run single-threaded.

mod support;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use sea_orm::ConnectionTrait;

use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::moderation::model::{
    MemberTimeout, MessageReportStatus, NewMessageReport, ServerBan,
};
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::moderation::SeaOrmModerationRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

#[tokio::test]
async fn bans_respect_expiry_upsert_and_delete() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmModerationRepository::new(ctx.db.clone());
    let server = ctx.create_server("bans").await;
    let moderator = ctx.create_user("moderator").await.id;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let now = Utc::now();

    repo.upsert_ban(ServerBan {
        server_id: server.id,
        user_id: alice,
        reason: Some("spam".to_string()),
        banned_by: Some(moderator),
        created_at: now,
        expires_at: None,
    })
    .await
    .unwrap();
    repo.upsert_ban(ServerBan {
        server_id: server.id,
        user_id: bob,
        reason: None,
        banned_by: Some(moderator),
        created_at: now,
        expires_at: Some(now + Duration::hours(1)),
    })
    .await
    .unwrap();

    assert!(
        repo.get_active_ban(server.id, alice, now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repo.get_active_ban(server.id, bob, now)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repo.get_active_ban(server.id, bob, now + Duration::hours(2))
            .await
            .unwrap()
            .is_none(),
        "an expired ban is ignored"
    );
    assert_eq!(
        repo.list_active_bans(server.id, now).await.unwrap().len(),
        2
    );
    assert_eq!(
        repo.list_active_bans(server.id, now + Duration::hours(2))
            .await
            .unwrap()
            .len(),
        1
    );

    // Upsert replaces the row (no duplicate-key error).
    let replaced = repo
        .upsert_ban(ServerBan {
            server_id: server.id,
            user_id: alice,
            reason: Some("raid".to_string()),
            banned_by: Some(moderator),
            created_at: now,
            expires_at: Some(now + Duration::days(1)),
        })
        .await
        .unwrap();
    assert_eq!(replaced.reason.as_deref(), Some("raid"));

    assert!(repo.delete_ban(server.id, alice).await.unwrap());
    assert!(!repo.delete_ban(server.id, alice).await.unwrap());
    assert!(
        repo.get_active_ban(server.id, alice, now)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn timeouts_expire_and_drive_member_standing() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmModerationRepository::new(ctx.db.clone());
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("timeouts").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    members
        .add_member(server.id, alice, ServerMemberRole::Member)
        .await
        .unwrap();
    members
        .add_member(server.id, bob, ServerMemberRole::Guest)
        .await
        .unwrap();

    let standing = members.get_member_standing(server.id, alice).await.unwrap();
    assert_eq!(
        standing.map(|s| (s.role, s.timed_out_until)),
        Some((ServerMemberRole::Member, None))
    );

    let until = Utc::now() + Duration::minutes(10);
    repo.upsert_timeout(MemberTimeout {
        server_id: server.id,
        user_id: alice,
        until,
        reason: None,
        set_by: None,
        created_at: Utc::now(),
    })
    .await
    .unwrap();
    // An already-expired timeout for bob is stored but never takes effect.
    repo.upsert_timeout(MemberTimeout {
        server_id: server.id,
        user_id: bob,
        until: Utc::now() - Duration::seconds(1),
        reason: None,
        set_by: None,
        created_at: Utc::now(),
    })
    .await
    .unwrap();

    let standing = members
        .get_member_standing(server.id, alice)
        .await
        .unwrap()
        .expect("member");
    let stored = standing.timed_out_until.expect("active timeout");
    assert!((stored - until).num_milliseconds().abs() < 1000);
    let bob_standing = members
        .get_member_standing(server.id, bob)
        .await
        .unwrap()
        .expect("member");
    assert_eq!(bob_standing.role, ServerMemberRole::Guest);
    assert_eq!(
        bob_standing.timed_out_until, None,
        "expired timeout ignored"
    );

    assert!(
        repo.get_active_timeout(server.id, alice, until + Duration::seconds(1))
            .await
            .unwrap()
            .is_none(),
        "the timeout expires at `until`"
    );
    let active = repo
        .list_active_timeouts(server.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].user_id, alice);

    assert!(repo.delete_timeout(server.id, alice).await.unwrap());
    assert!(
        members
            .get_member_standing(server.id, alice)
            .await
            .unwrap()
            .expect("member")
            .timed_out_until
            .is_none()
    );
    assert!(
        members
            .get_member_standing(server.id, ctx.create_user("carol").await.id)
            .await
            .unwrap()
            .is_none(),
        "non-members have no standing"
    );
}

#[tokio::test]
async fn reports_are_unique_paginated_and_resolved_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmModerationRepository::new(ctx.db.clone());
    let server = ctx.create_server("reports").await;
    let author = ctx.create_user("author").await.id;
    let reporter = ctx.create_user("reporter").await.id;
    let moderator = ctx.create_user("moderator").await.id;
    let channel = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let mut reports = Vec::new();
    for i in 0..3 {
        let message = ctx
            .create_message(channel.id, author, &format!("m{i}"))
            .await;
        reports.push(
            repo.create_report(NewMessageReport {
                server_id: server.id,
                conversation_id: channel.id,
                message_id: message.id,
                reporter_user_id: reporter,
                reason: format!("reason {i}"),
                franked: None,
            })
            .await
            .unwrap(),
        );
    }

    let duplicate = repo
        .create_report(NewMessageReport {
            server_id: server.id,
            conversation_id: channel.id,
            message_id: reports[0].message_id,
            reporter_user_id: reporter,
            reason: "again".to_string(),
            franked: None,
        })
        .await
        .unwrap_err();
    assert_eq!(duplicate.kind(), RepoErrorKind::Conflict);

    // Pagination visits every report exactly once, newest first (a DESC endpoint: `After`
    // walks toward older reports, `Before` toward newer ones).
    let first = repo
        .list_reports(server.id, None, 2, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = repo
        .list_reports(server.id, None, 2, PageDir::After, first.next)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next.is_none());
    let back = repo
        .list_reports(server.id, None, 2, PageDir::Before, second.prev)
        .await
        .unwrap();
    assert_eq!(
        back.items.iter().map(|r| r.id).collect::<Vec<_>>(),
        first.items.iter().map(|r| r.id).collect::<Vec<_>>()
    );
    let ids: Vec<_> = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, reports.iter().rev().map(|r| r.id).collect::<Vec<_>>());

    let resolved = repo
        .resolve_report(
            reports[1].id,
            MessageReportStatus::Resolved,
            moderator,
            Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status, MessageReportStatus::Resolved);
    assert_eq!(resolved.resolved_by, Some(moderator));
    let again = repo
        .resolve_report(
            reports[1].id,
            MessageReportStatus::Dismissed,
            moderator,
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(again.kind(), RepoErrorKind::PreconditionFailed);
    let missing = repo
        .resolve_report(
            rim_core::moderation::model::MessageReportId::new(),
            MessageReportStatus::Dismissed,
            moderator,
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), RepoErrorKind::NotFound);

    let open = repo
        .list_reports(
            server.id,
            Some(MessageReportStatus::Open),
            10,
            PageDir::After,
            None,
        )
        .await
        .unwrap();
    assert_eq!(open.items.len(), 2);
    assert!(repo.get_report(reports[0].id).await.unwrap().is_some());
}

#[tokio::test]
async fn slow_mode_column_and_last_sent_at() {
    let ctx = support::TestCtx::new().await;
    let conversations = SeaOrmConversationRepository::new(ctx.db.clone());
    let messages = SeaOrmMessageRepository::new(ctx.db.clone());
    let server = ctx.create_server("slow").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let channel = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    assert_eq!(channel.slow_mode_seconds, 0);

    let updated = conversations.set_slow_mode(channel.id, 30).await.unwrap();
    assert_eq!(updated.slow_mode_seconds, 30);
    assert_eq!(
        conversations
            .get_by_id(channel.id)
            .await
            .unwrap()
            .expect("channel")
            .slow_mode_seconds,
        30
    );
    // The CHECK constraint backs the service-level range check.
    assert!(
        conversations
            .set_slow_mode(channel.id, 21_601)
            .await
            .is_err()
    );

    assert_eq!(
        messages
            .last_sent_at_by_author(channel.id, alice)
            .await
            .unwrap(),
        None
    );
    ctx.create_message(channel.id, alice, "one").await;
    let second = ctx.create_message(channel.id, alice, "two").await;
    ctx.create_message(channel.id, bob, "bob").await;
    let last = messages
        .last_sent_at_by_author(channel.id, alice)
        .await
        .unwrap()
        .expect("alice wrote");
    assert_eq!(last, second.sent_at);
}

/// The `add_member_role` migration's data step: every `guest` that existed before it becomes a
/// `member`, and new rows default to `member`.
#[tokio::test]
async fn role_migration_turns_existing_guests_into_members() {
    const MIGRATION_NAME: &str = "m20261005_024730_add_member_role";
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;

    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");

    // Users and the server are created on the current schema: the entities know columns that
    // later migrations added, so inserting through them after the rollback would fail.
    let server = ctx.create_server("pre-roles").await;
    let owner = ctx.create_user("owner").await.id;
    let guest = ctx.create_user("old-guest").await.id;

    Migrator::down(&conn, Some(steps))
        .await
        .expect("roll back to before add_member_role");

    // Seed with raw SQL: the old three-value enum has no `member`.
    for (user, role) in [(owner, "owner"), (guest, "guest")] {
        conn.execute_unprepared(&format!(
            "INSERT INTO server_member (id, server_id, user_id, member_role) \
             VALUES (gen_random_uuid(), '{}', '{}', '{role}')",
            uuid::Uuid::from(server.id),
            uuid::Uuid::from(user),
        ))
        .await
        .expect("seed member");
    }

    Migrator::up(&conn, None)
        .await
        .expect("re-apply migrations");

    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let role = |user| {
        let members = &members;
        let server_id = server.id;
        async move {
            members
                .get_member(server_id, user)
                .await
                .unwrap()
                .expect("member")
                .member_role
        }
    };
    assert_eq!(role(owner).await, ServerMemberRole::Owner);
    assert_eq!(
        role(guest).await,
        ServerMemberRole::Member,
        "guest -> member"
    );

    // The column default is now `member`.
    let newcomer = ctx.create_user("newcomer").await.id;
    conn.execute_unprepared(&format!(
        "INSERT INTO server_member (id, server_id, user_id) VALUES (gen_random_uuid(), '{}', '{}')",
        uuid::Uuid::from(server.id),
        uuid::Uuid::from(newcomer),
    ))
    .await
    .expect("insert with default role");
    assert_eq!(role(newcomer).await, ServerMemberRole::Member);
}
