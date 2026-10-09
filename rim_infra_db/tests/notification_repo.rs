#![cfg(feature = "db-tests")]

//! Track 1.3: notification settings, the notification inbox (paging, unique `(user, message)`,
//! mark-read paths), the digest candidate query, retention, level overrides and the permanent
//! mute data step against a live Postgres. Destructive for the shared test database: run
//! single-threaded.

mod support;

use chrono::{DateTime, Duration, NaiveTime, Utc};
use sea_orm::ConnectionTrait;

use rim_core::conversation::model::{Conversation, ConversationType};
use rim_core::message::repository::MessageRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::notification::model::{
    DigestFrequency, DndPause, DndWindow, NewUserNotification, NotificationLevel,
    NotificationReason, UserNotificationSettings,
};
use rim_core::notification::repository::{ListUserNotificationsParams, NotificationRepository};
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::notification::SeaOrmNotificationRepository;
use rim_infra_db::sea_orm_impl::notification_preference::SeaOrmNotificationPreferenceRepository;

/// Timestamps truncated to whole microseconds, as Postgres stores them.
fn t(offset_minutes: i64) -> DateTime<Utc> {
    let now = Utc::now() + Duration::minutes(offset_minutes);
    DateTime::from_timestamp_micros(now.timestamp_micros()).expect("valid timestamp")
}

fn new_row(
    user_id: UserId,
    server_id: ServerId,
    conversation: &Conversation,
    message_id: rim_core::message::model::MessageId,
    reason: NotificationReason,
) -> NewUserNotification {
    NewUserNotification {
        user_id,
        server_id,
        conversation_id: conversation.id,
        message_id,
        reason,
    }
}

fn settings(
    user_id: UserId,
    server_id: ServerId,
    digest: DigestFrequency,
) -> UserNotificationSettings {
    UserNotificationSettings {
        digest,
        ..UserNotificationSettings::defaults(user_id, server_id, t(0))
    }
}

#[tokio::test]
async fn settings_round_trip_and_batch_lookup() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("settings").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;

    assert!(repo.get_settings(alice, server.id).await.unwrap().is_none());

    let saved = UserNotificationSettings {
        user_id: alice,
        server_id: server.id,
        default_level: NotificationLevel::Mentions,
        keywords: vec!["deploy".to_string(), "on-call".to_string()],
        dnd: Some(DndWindow {
            start: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(7, 30, 0).unwrap(),
            timezone: "Europe/Prague".to_string(),
        }),
        dnd_pause: DndPause::Until(t(60)),
        digest: DigestFrequency::Daily,
        updated_at: t(0),
    };
    assert_eq!(repo.upsert_settings(saved.clone()).await.unwrap(), saved);
    assert_eq!(
        repo.get_settings(alice, server.id).await.unwrap(),
        Some(saved.clone())
    );

    // Upsert replaces every column, including clearing the window and the pause.
    let replaced = UserNotificationSettings {
        dnd: None,
        dnd_pause: DndPause::Indefinite,
        keywords: vec![],
        ..saved.clone()
    };
    repo.upsert_settings(replaced.clone()).await.unwrap();
    assert_eq!(
        repo.get_settings(alice, server.id).await.unwrap(),
        Some(replaced)
    );

    let batch = repo
        .get_settings_for_users(server.id, &[alice, bob])
        .await
        .unwrap();
    assert_eq!(batch.len(), 1, "bob has no row");
    assert_eq!(batch[0].user_id, alice);
    assert!(
        repo.get_settings_for_users(server.id, &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn one_notification_per_user_and_message() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("unique").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let author = ctx.create_user("author").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let msg = ctx.create_message(conv.id, author, "hello").await;

    let inserted = repo
        .create_notifications(
            &[
                new_row(alice, server.id, &conv, msg.id, NotificationReason::Mention),
                new_row(bob, server.id, &conv, msg.id, NotificationReason::All),
            ],
            t(0),
        )
        .await
        .unwrap();
    assert_eq!(inserted.len(), 2);
    assert!(
        inserted
            .iter()
            .all(|n| n.read_at.is_none() && n.emailed_at.is_none())
    );

    // A second decision for the same message inserts (and returns) nothing for alice.
    let again = repo
        .create_notifications(
            &[new_row(
                alice,
                server.id,
                &conv,
                msg.id,
                NotificationReason::Keyword,
            )],
            t(1),
        )
        .await
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(repo.unread_count(alice, server.id).await.unwrap(), 1);
    let page = repo
        .list_notifications(ListUserNotificationsParams {
            user_id: alice,
            server_id: server.id,
            unread_only: false,
            limit: 10,
            dir: PageDir::After,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].reason,
        NotificationReason::Mention,
        "first reason wins"
    );
    assert!(
        repo.create_notifications(&[], t(0))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn notifications_page_newest_first_with_standard_cursors() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("paging").await;
    let other_server = ctx.create_server("other").await;
    let alice = ctx.create_user("alice").await.id;
    let author = ctx.create_user("author").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let mut ids = vec![];
    for i in 0..5 {
        let msg = ctx.create_message(conv.id, author, &format!("m{i}")).await;
        let rows = repo
            .create_notifications(
                &[new_row(
                    alice,
                    server.id,
                    &conv,
                    msg.id,
                    NotificationReason::All,
                )],
                t(-60 + i),
            )
            .await
            .unwrap();
        ids.push(rows[0].id);
    }
    // Another workspace's notification never shows up.
    let other_conv = ctx
        .create_conversation(other_server.id, ConversationType::Channel)
        .await;
    let other_msg = ctx.create_message(other_conv.id, author, "elsewhere").await;
    repo.create_notifications(
        &[new_row(
            alice,
            other_server.id,
            &other_conv,
            other_msg.id,
            NotificationReason::All,
        )],
        t(0),
    )
    .await
    .unwrap();

    let params = |dir, cursor, unread_only| ListUserNotificationsParams {
        user_id: alice,
        server_id: server.id,
        unread_only,
        limit: 2,
        dir,
        cursor,
    };
    let first = repo
        .list_notifications(params(PageDir::After, None, false))
        .await
        .unwrap();
    assert_eq!(
        first.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![ids[4], ids[3]]
    );
    assert!(first.prev.is_none());
    let next = first.next.expect("more pages");

    let second = repo
        .list_notifications(params(PageDir::After, Some(next), false))
        .await
        .unwrap();
    assert_eq!(
        second.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]]
    );
    let third = repo
        .list_notifications(params(PageDir::After, second.next, false))
        .await
        .unwrap();
    assert_eq!(
        third.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![ids[0]]
    );
    assert!(third.next.is_none());
    let back = third.prev.expect("a page back");

    // `Before` the oldest item = the two nearest newer ones, still newest first.
    let newer = repo
        .list_notifications(params(PageDir::Before, Some(back), false))
        .await
        .unwrap();
    assert_eq!(
        newer.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]]
    );
    assert!(newer.next.is_some(), "still newer ones beyond");

    // unread_only drops read rows.
    repo.mark_read_by_ids(alice, server.id, &[ids[4], ids[3]], t(0))
        .await
        .unwrap();
    let unread = repo
        .list_notifications(ListUserNotificationsParams {
            limit: 10,
            ..params(PageDir::After, None, true)
        })
        .await
        .unwrap();
    assert_eq!(
        unread.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1], ids[0]]
    );
}

#[tokio::test]
async fn mark_read_paths_are_scoped_and_idempotent() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("mark").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let author = ctx.create_user("author").await.id;
    let general = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let random = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let m1 = ctx.create_message(general.id, author, "one").await;
    let m2 = ctx.create_message(general.id, author, "two").await;
    let m3 = ctx.create_message(random.id, author, "three").await;
    let rows = repo
        .create_notifications(
            &[
                new_row(alice, server.id, &general, m1.id, NotificationReason::All),
                new_row(alice, server.id, &general, m2.id, NotificationReason::All),
                new_row(alice, server.id, &random, m3.id, NotificationReason::All),
                new_row(bob, server.id, &general, m1.id, NotificationReason::All),
            ],
            t(-30),
        )
        .await
        .unwrap();
    let bob_row = rows.iter().find(|n| n.user_id == bob).unwrap().id;
    let alice_m1 = rows
        .iter()
        .find(|n| n.user_id == alice && n.message_id == m1.id)
        .unwrap()
        .id;

    // Bob's id in alice's call is ignored.
    assert_eq!(
        repo.mark_read_by_ids(alice, server.id, &[alice_m1, bob_row], t(0))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.mark_read_by_ids(alice, server.id, &[alice_m1], t(0))
            .await
            .unwrap(),
        0,
        "already read"
    );
    assert_eq!(repo.unread_count(bob, server.id).await.unwrap(), 1);
    assert_eq!(repo.unread_count(alice, server.id).await.unwrap(), 2);

    // Reading `general` up to now clears m2 there, not `random`.
    assert_eq!(
        repo.mark_read_for_conversation(alice, general.id, t(1), t(1))
            .await
            .unwrap(),
        1
    );
    // A read position before the message leaves it unread.
    assert_eq!(
        repo.mark_read_for_conversation(alice, random.id, t(-120), t(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(repo.unread_count(alice, server.id).await.unwrap(), 1);

    // all-before: created before the cutoff only.
    assert_eq!(
        repo.mark_read_before(alice, server.id, t(-60), t(0))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        repo.mark_read_before(alice, server.id, t(0), t(0))
            .await
            .unwrap(),
        1
    );
    assert_eq!(repo.unread_count(alice, server.id).await.unwrap(), 0);
    assert_eq!(repo.unread_count(bob, server.id).await.unwrap(), 1);
}

#[tokio::test]
async fn deleting_a_message_drops_unread_rows_and_retention_purges_old_ones() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("delete").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let author = ctx.create_user("author").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let msg = ctx.create_message(conv.id, author, "oops").await;
    let rows = repo
        .create_notifications(
            &[
                new_row(alice, server.id, &conv, msg.id, NotificationReason::All),
                new_row(bob, server.id, &conv, msg.id, NotificationReason::All),
            ],
            t(0),
        )
        .await
        .unwrap();
    let alice_row = rows.iter().find(|n| n.user_id == alice).unwrap().id;
    repo.mark_read_by_ids(alice, server.id, &[alice_row], t(0))
        .await
        .unwrap();

    assert_eq!(repo.delete_unread_for_message(msg.id).await.unwrap(), 1);
    assert_eq!(repo.unread_count(bob, server.id).await.unwrap(), 0);
    let alice_page = repo
        .list_notifications(ListUserNotificationsParams {
            user_id: alice,
            server_id: server.id,
            unread_only: false,
            limit: 10,
            dir: PageDir::After,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(alice_page.items.len(), 1, "the read row stays");

    // Retention: an old row goes, a recent one stays.
    let old_msg = ctx.create_message(conv.id, author, "ancient").await;
    repo.create_notifications(
        &[new_row(
            bob,
            server.id,
            &conv,
            old_msg.id,
            NotificationReason::All,
        )],
        t(-91 * 24 * 60),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.delete_created_before(t(-90 * 24 * 60)).await.unwrap(),
        1
    );
    assert_eq!(repo.unread_count(bob, server.id).await.unwrap(), 0);
    assert_eq!(
        repo.delete_created_before(t(-90 * 24 * 60)).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn digest_candidates_follow_frequency_age_and_last_digest() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("Acme").await;
    let author = ctx.create_user("author").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    // (name, frequency, pending age in minutes, last digest minutes ago, read?)
    let cases: [(&str, DigestFrequency, i64, Option<i64>, bool); 7] = [
        ("hourly_due", DigestFrequency::Hourly, 90, None, false),
        ("hourly_young", DigestFrequency::Hourly, 30, None, false),
        (
            "hourly_recent_digest",
            DigestFrequency::Hourly,
            90,
            Some(20),
            false,
        ),
        ("daily_young", DigestFrequency::Daily, 120, None, false),
        (
            "daily_due",
            DigestFrequency::Daily,
            25 * 60,
            Some(26 * 60),
            false,
        ),
        ("off", DigestFrequency::Off, 25 * 60, None, false),
        ("hourly_read", DigestFrequency::Hourly, 90, None, true),
    ];
    let mut users = std::collections::HashMap::new();
    for (name, frequency, age, last_digest, read) in cases {
        let user = ctx.create_user(name).await.id;
        users.insert(name, user);
        repo.upsert_settings(settings(user, server.id, frequency))
            .await
            .unwrap();
        let msg = ctx.create_message(conv.id, author, name).await;
        let rows = repo
            .create_notifications(
                &[new_row(
                    user,
                    server.id,
                    &conv,
                    msg.id,
                    NotificationReason::Mention,
                )],
                t(-age),
            )
            .await
            .unwrap();
        if read {
            repo.mark_read_by_ids(user, server.id, &[rows[0].id], t(0))
                .await
                .unwrap();
        }
        if let Some(ago) = last_digest {
            // An older, already-emailed notification records the previous digest.
            let earlier = ctx.create_message(conv.id, author, "earlier").await;
            let prev = repo
                .create_notifications(
                    &[new_row(
                        user,
                        server.id,
                        &conv,
                        earlier.id,
                        NotificationReason::All,
                    )],
                    t(-ago - 5),
                )
                .await
                .unwrap();
            repo.mark_emailed(&[prev[0].id], t(-ago)).await.unwrap();
        }
    }

    let now = t(0);
    let candidates = repo
        .list_digest_candidates(now - Duration::minutes(60), now - Duration::hours(24))
        .await
        .unwrap();
    let mut names: Vec<&str> = candidates
        .iter()
        .map(|c| {
            *users
                .iter()
                .find(|(_, id)| **id == c.user_id)
                .map(|(name, _)| name)
                .unwrap()
        })
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["daily_due", "hourly_due"]);

    let daily = candidates
        .iter()
        .find(|c| c.user_id == users["daily_due"])
        .unwrap();
    assert_eq!(daily.frequency, DigestFrequency::Daily);
    assert_eq!(daily.email, "daily_due@test.example.com");
    assert_eq!(daily.server_name, "Acme");
    assert_eq!(daily.pending_count, 1);
    assert!(daily.last_digest_at.is_some());
    let hourly = candidates
        .iter()
        .find(|c| c.user_id == users["hourly_due"])
        .unwrap();
    assert!(hourly.last_digest_at.is_none());

    // The pending list is oldest first; once emailed, the candidate disappears.
    let pending = repo
        .list_pending_digest(hourly.user_id, server.id, 50)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(repo.mark_emailed(&[pending[0].id], now).await.unwrap(), 1);
    let after = repo
        .list_digest_candidates(now - Duration::minutes(60), now - Duration::hours(24))
        .await
        .unwrap();
    assert!(after.iter().all(|c| c.user_id != hourly.user_id));
}

/// A key-only account has no email address, so it is never a digest candidate, however due.
#[tokio::test]
async fn digest_candidates_skip_users_without_email() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationRepository::new(ctx.db.clone());
    let server = ctx.create_server("Acme").await;
    let author = ctx.create_user("author").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let with_email = ctx.create_user("with_email").await.id;
    let key_only = ctx
        .create_key_only_user("key_only", "rimp1_digest")
        .await
        .id;
    for user in [with_email, key_only] {
        repo.upsert_settings(settings(user, server.id, DigestFrequency::Hourly))
            .await
            .unwrap();
        let msg = ctx.create_message(conv.id, author, "hello").await;
        repo.create_notifications(
            &[new_row(
                user,
                server.id,
                &conv,
                msg.id,
                NotificationReason::Mention,
            )],
            t(-90),
        )
        .await
        .unwrap();
    }

    let now = t(0);
    let candidates = repo
        .list_digest_candidates(now - Duration::minutes(60), now - Duration::hours(24))
        .await
        .unwrap();
    let users: Vec<UserId> = candidates.iter().map(|c| c.user_id).collect();
    assert_eq!(users, vec![with_email]);
}

#[tokio::test]
async fn level_overrides_and_mutes_share_one_row() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationPreferenceRepository::new(ctx.db.clone());
    let server = ctx.create_server("levels").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    // A level override alone is not a mute.
    let pref = repo
        .set_level(alice, conv.id, Some(NotificationLevel::Mentions))
        .await
        .unwrap()
        .expect("row");
    assert_eq!(pref.level, Some(NotificationLevel::Mentions));
    assert!(!repo.is_muted(alice, conv.id).await.unwrap());
    assert!(
        repo.get_muted_conversations(alice)
            .await
            .unwrap()
            .is_empty()
    );

    // A timed mute keeps the level; unmuting keeps it too.
    let pref = repo
        .set_mute(alice, conv.id, Some(Utc::now() + Duration::hours(1)))
        .await
        .unwrap();
    assert_eq!(pref.level, Some(NotificationLevel::Mentions));
    assert!(repo.is_muted(alice, conv.id).await.unwrap());
    assert_eq!(repo.get_muted_conversations(alice).await.unwrap().len(), 1);
    repo.remove_mute(alice, conv.id).await.unwrap();
    let pref = repo
        .get_preference(alice, conv.id)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(pref.level, Some(NotificationLevel::Mentions));
    assert_eq!(pref.muted_until, None);

    // A permanent mute is level NONE; unmute clears it and the empty row goes away.
    let pref = repo.set_mute(alice, conv.id, None).await.unwrap();
    assert_eq!(pref.level, Some(NotificationLevel::None));
    assert!(repo.is_muted(alice, conv.id).await.unwrap());
    repo.remove_mute(alice, conv.id).await.unwrap();
    assert!(repo.get_preference(alice, conv.id).await.unwrap().is_none());

    // A timed mute over a NONE level replaces it, so the mute really ends.
    repo.set_level(alice, conv.id, Some(NotificationLevel::None))
        .await
        .unwrap();
    let pref = repo
        .set_mute(alice, conv.id, Some(Utc::now() - Duration::minutes(1)))
        .await
        .unwrap();
    assert_eq!(pref.level, None);
    assert!(!repo.is_muted(alice, conv.id).await.unwrap(), "expired");

    // Clearing the level of a row with no mute deletes it.
    repo.remove_mute(alice, conv.id).await.unwrap();
    repo.set_level(alice, conv.id, Some(NotificationLevel::All))
        .await
        .unwrap();
    assert!(
        repo.set_level(alice, conv.id, None)
            .await
            .unwrap()
            .is_none()
    );

    repo.set_level(bob, conv.id, Some(NotificationLevel::None))
        .await
        .unwrap();
    let prefs = repo
        .list_for_conversation(conv.id, &[alice, bob])
        .await
        .unwrap();
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0].user_id, bob);
}

/// The migration's data step turns a pre-Track-1.3 permanent mute into level NONE.
#[tokio::test]
async fn permanent_mute_data_step_sets_level_none() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmNotificationPreferenceRepository::new(ctx.db.clone());
    let server = ctx.create_server("legacy").await;
    let alice = ctx.create_user("alice").await.id;
    let bob = ctx.create_user("bob").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let until = Utc::now() + Duration::hours(1);
    ctx.db
        .execute_unprepared(&format!(
            "INSERT INTO notification_preference (user_id, conversation_id, muted_until) VALUES \
             ('{alice}', '{conv}', NULL), ('{bob}', '{conv}', '{until}')",
            conv = conv.id,
            until = until.to_rfc3339(),
        ))
        .await
        .unwrap();
    ctx.db
        .execute_unprepared(migration::PERMANENT_MUTE_TO_NONE_SQL)
        .await
        .unwrap();
    let alice_pref = repo.get_preference(alice, conv.id).await.unwrap().unwrap();
    assert_eq!(alice_pref.level, Some(NotificationLevel::None));
    let bob_pref = repo.get_preference(bob, conv.id).await.unwrap().unwrap();
    assert_eq!(bob_pref.level, None, "timed mutes keep inheriting");
    assert!(repo.is_muted(alice, conv.id).await.unwrap());
    assert!(repo.is_muted(bob, conv.id).await.unwrap());
}

#[tokio::test]
async fn thread_participants_are_root_author_and_repliers() {
    let ctx = support::TestCtx::new().await;
    let messages = SeaOrmMessageRepository::new(ctx.db.clone());
    let threads = SeaOrmMessageThreadRepository::new(ctx.db.clone());
    let server = ctx.create_server("threads").await;
    let root_author = ctx.create_user("root").await.id;
    let replier = ctx.create_user("replier").await.id;
    let deleted_replier = ctx.create_user("deleted").await.id;
    let bystander = ctx.create_user("bystander").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let root = ctx.create_message(conv.id, root_author, "root").await;
    ctx.create_message(conv.id, bystander, "unrelated").await;
    let thread = threads
        .create_thread_for_root(conv.id, root.id)
        .await
        .unwrap();
    ctx.create_thread_message(conv.id, thread.id, replier, "r1")
        .await;
    ctx.create_thread_message(conv.id, thread.id, replier, "r2")
        .await;
    let gone = ctx
        .create_thread_message(conv.id, thread.id, deleted_replier, "r3")
        .await;
    messages.soft_delete_message(gone.id).await.unwrap();

    let mut ids = messages
        .list_thread_participant_ids(thread.id)
        .await
        .unwrap();
    ids.sort_by_key(|id| id.to_string());
    let mut expected = vec![root_author, replier];
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(ids, expected);
}
