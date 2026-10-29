#![cfg(feature = "db-tests")]

//! One author's messages in a workspace's channels: the counts split by whether the viewer
//! reads the channel, the channel list a bulk delete walks, and the per-channel soft delete,
//! against a live Postgres. Destructive for the shared test database: run single-threaded.

mod support;

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use sea_orm::ConnectionTrait;

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::model::MessageId;
use rim_core::message::repository::{
    AuthorChannelCounts, AuthorChannelScope, MessageRepository, MessageTally,
};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;

async fn exec(ctx: &support::TestCtx, sql: String) {
    ctx.db.execute_unprepared(&sql).await.unwrap();
}

/// A channel of `server_id`, private when asked.
async fn channel(ctx: &support::TestCtx, server_id: ServerId, private: bool) -> Conversation {
    let conv = ctx
        .create_conversation(server_id, ConversationType::Channel)
        .await;
    if private {
        exec(
            ctx,
            format!(
                "UPDATE conversation SET is_private = true WHERE id = '{}'",
                conv.id
            ),
        )
        .await;
    }
    conv
}

/// Moves a message's `sent_at` back by `minutes`.
async fn back_date(ctx: &support::TestCtx, id: MessageId, minutes: i64) {
    exec(
        ctx,
        format!(
            "UPDATE message SET sent_at = now() - interval '{minutes} minutes' WHERE id = '{id}'"
        ),
    )
    .await;
}

fn scope(
    server_id: ServerId,
    author: UserId,
    viewer: UserId,
    since: Option<DateTime<Utc>>,
) -> AuthorChannelScope {
    AuthorChannelScope {
        server_id,
        author,
        since,
        viewer,
    }
}

fn tally(messages: u64, channels: u64) -> MessageTally {
    MessageTally { messages, channels }
}

#[tokio::test]
async fn counts_split_readable_and_unjoined_private_channels() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let members = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("acme").await;
    let bob = ctx.create_user("bob").await.id;
    let mona = ctx.create_user("mona").await.id;

    let general = channel(&ctx, server.id, false).await;
    let random = channel(&ctx, server.id, false).await;
    let staff = channel(&ctx, server.id, true).await;
    let secret = channel(&ctx, server.id, true).await;
    let invited = channel(&ctx, server.id, true).await;
    members.add_member(staff.id, mona).await.unwrap();
    members.add_pending_member(invited.id, mona).await.unwrap();

    let thread = ctx.create_thread(general.id).await;
    ctx.create_message(general.id, bob, "one").await;
    ctx.create_message(general.id, bob, "two").await;
    ctx.create_thread_message(general.id, thread.id, bob, "reply")
        .await;
    ctx.create_message(general.id, mona, "not bob").await;
    ctx.create_message(random.id, bob, "three").await;
    ctx.create_message(staff.id, bob, "four").await;
    ctx.create_message(secret.id, bob, "five").await;
    ctx.create_message(secret.id, bob, "six").await;
    ctx.create_message(invited.id, bob, "seven").await;

    let counts = repo
        .count_author_channel_messages(scope(server.id, bob, mona, None))
        .await
        .unwrap();
    assert_eq!(
        counts,
        AuthorChannelCounts {
            readable: tally(5, 3),
            unjoined_private: tally(3, 2),
        },
        "a pending membership does not make a private channel readable"
    );

    let nobody = repo
        .count_author_channel_messages(scope(server.id, mona, bob, None))
        .await
        .unwrap();
    assert_eq!(nobody.readable, tally(1, 1));
    assert_eq!(nobody.unjoined_private, tally(0, 0));
}

#[tokio::test]
async fn counts_skip_dms_groups_archived_encrypted_deleted_and_older_messages() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let server = ctx.create_server("acme").await;
    let elsewhere = ctx.create_server("other").await;
    let bob = ctx.create_user("bob").await.id;
    let mona = ctx.create_user("mona").await.id;

    let general = channel(&ctx, server.id, false).await;
    let dm = ctx
        .create_conversation(server.id, ConversationType::Direct)
        .await;
    let group = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let archived = channel(&ctx, server.id, false).await;
    let encrypted = channel(&ctx, server.id, false).await;
    let other_workspace = channel(&ctx, elsewhere.id, false).await;
    exec(
        &ctx,
        format!(
            "UPDATE conversation SET archived_at = now() WHERE id = '{}'",
            archived.id
        ),
    )
    .await;
    exec(
        &ctx,
        format!(
            "UPDATE conversation SET encrypted_at = now() WHERE id = '{}'",
            encrypted.id
        ),
    )
    .await;

    ctx.create_message(general.id, bob, "recent").await;
    let old = ctx.create_message(general.id, bob, "old").await;
    back_date(&ctx, old.id, 2 * 24 * 60).await;
    let deleted = ctx.create_message(general.id, bob, "deleted").await;
    repo.soft_delete_message(deleted.id).await.unwrap();
    let sealed = ctx.create_message(general.id, bob, "sealed").await;
    exec(
        &ctx,
        format!(
            "UPDATE message SET encrypted = true, text = NULL, mls_epoch = 1, \
             mls_incarnation = 1 WHERE id = '{}'",
            sealed.id
        ),
    )
    .await;
    for conv in [&dm, &group, &archived, &encrypted, &other_workspace] {
        ctx.create_message(conv.id, bob, "out of scope").await;
    }

    let all = repo
        .count_author_channel_messages(scope(server.id, bob, mona, None))
        .await
        .unwrap();
    assert_eq!(all.readable, tally(2, 1));
    assert_eq!(all.unjoined_private, tally(0, 0));

    let last_hour = repo
        .count_author_channel_messages(scope(
            server.id,
            bob,
            mona,
            Some(Utc::now() - Duration::hours(1)),
        ))
        .await
        .unwrap();
    assert_eq!(last_hour.readable, tally(1, 1));

    let none = repo
        .list_author_channels(scope(server.id, bob, mona, None), true)
        .await
        .unwrap();
    assert_eq!(none, [general.id]);
}

#[tokio::test]
async fn list_author_channels_orders_by_newest_message_and_honours_the_include_flag() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let server = ctx.create_server("acme").await;
    let bob = ctx.create_user("bob").await.id;
    let mona = ctx.create_user("mona").await.id;

    let a = channel(&ctx, server.id, false).await;
    let b = channel(&ctx, server.id, false).await;
    let secret = channel(&ctx, server.id, true).await;
    let quiet = channel(&ctx, server.id, false).await;

    let a_old = ctx.create_message(a.id, bob, "a old").await;
    let a_new = ctx.create_message(a.id, bob, "a new").await;
    let b_only = ctx.create_message(b.id, bob, "b").await;
    let s_only = ctx.create_message(secret.id, bob, "secret").await;
    ctx.create_message(quiet.id, mona, "someone else").await;
    back_date(&ctx, a_old.id, 180).await;
    back_date(&ctx, a_new.id, 10).await;
    back_date(&ctx, b_only.id, 60).await;
    back_date(&ctx, s_only.id, 5).await;

    let readable = repo
        .list_author_channels(scope(server.id, bob, mona, None), false)
        .await
        .unwrap();
    assert_eq!(readable, [a.id, b.id]);
    let everything = repo
        .list_author_channels(scope(server.id, bob, mona, None), true)
        .await
        .unwrap();
    assert_eq!(everything, [secret.id, a.id, b.id]);

    let since = repo
        .list_author_channels(
            scope(
                server.id,
                bob,
                mona,
                Some(Utc::now() - Duration::minutes(30)),
            ),
            false,
        )
        .await
        .unwrap();
    assert_eq!(since, [a.id], "only channels with a message since `since`");
}

#[tokio::test]
async fn soft_delete_author_channel_messages_deletes_newest_first_up_to_the_limit_and_bumps_the_feed()
 {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let server = ctx.create_server("acme").await;
    let bob = ctx.create_user("bob").await.id;
    let alice = ctx.create_user("alice").await.id;
    let general = channel(&ctx, server.id, false).await;
    let thread = ctx.create_thread(general.id).await;

    let m1 = ctx.create_message(general.id, bob, "m1").await;
    let m2 = ctx.create_message(general.id, bob, "m2").await;
    let reply = ctx
        .create_thread_message(general.id, thread.id, bob, "reply")
        .await;
    let m3 = ctx.create_message(general.id, bob, "m3").await;
    let theirs = ctx.create_message(general.id, alice, "alice").await;
    back_date(&ctx, m1.id, 240).await;
    back_date(&ctx, m2.id, 180).await;
    back_date(&ctx, reply.id, 120).await;
    back_date(&ctx, m3.id, 60).await;
    let before = repo.latest_change_version(general.id).await.unwrap();

    let first = repo
        .soft_delete_author_channel_messages(general.id, bob, None, 2)
        .await
        .unwrap();
    let ids: HashSet<MessageId> = first.iter().map(|m| m.id).collect();
    assert_eq!(ids, HashSet::from([m3.id, reply.id]), "newest first");
    let reply_row = first.iter().find(|m| m.id == reply.id).unwrap();
    assert_eq!(reply_row.thread_id, Some(thread.id));
    assert!(
        first
            .iter()
            .find(|m| m.id == m3.id)
            .unwrap()
            .thread_id
            .is_none()
    );

    let changes = repo
        .list_message_changes(general.id, before, 10)
        .await
        .unwrap();
    let changed: HashSet<MessageId> = changes.iter().map(|c| c.message.id).collect();
    assert_eq!(changed, HashSet::from([m3.id, reply.id]));
    assert!(
        changes
            .iter()
            .all(|c| c.version > before && c.message.is_deleted)
    );

    let since = Utc::now() - Duration::minutes(210);
    let second = repo
        .soft_delete_author_channel_messages(general.id, bob, Some(since), 10)
        .await
        .unwrap();
    assert_eq!(
        second.iter().map(|m| m.id).collect::<Vec<_>>(),
        [m2.id],
        "a message before `since` stays"
    );
    assert!(
        repo.soft_delete_author_channel_messages(general.id, bob, Some(since), 10)
            .await
            .unwrap()
            .is_empty()
    );

    assert!(!repo.get_by_id(m1.id).await.unwrap().unwrap().is_deleted);
    assert!(!repo.get_by_id(theirs.id).await.unwrap().unwrap().is_deleted);
    assert!(repo.get_by_id(m2.id).await.unwrap().unwrap().is_deleted);
}

#[tokio::test]
async fn soft_delete_author_channel_messages_skips_an_archived_channel() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let server = ctx.create_server("acme").await;
    let bob = ctx.create_user("bob").await.id;
    let archived = channel(&ctx, server.id, false).await;
    let dm = ctx
        .create_conversation(server.id, ConversationType::Direct)
        .await;
    let in_archive = ctx.create_message(archived.id, bob, "archived").await;
    let in_dm = ctx.create_message(dm.id, bob, "dm").await;
    exec(
        &ctx,
        format!(
            "UPDATE conversation SET archived_at = now() WHERE id = '{}'",
            archived.id
        ),
    )
    .await;

    let targets: [(ConversationId, MessageId); 2] =
        [(archived.id, in_archive.id), (dm.id, in_dm.id)];
    for (conv, message) in targets {
        assert!(
            repo.soft_delete_author_channel_messages(conv, bob, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!repo.get_by_id(message).await.unwrap().unwrap().is_deleted);
    }
}
