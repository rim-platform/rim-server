#![cfg(feature = "db-tests")]

mod support;

use sea_orm::ConnectionTrait;

use rim_core::conversation::model::{ConversationId, ConversationType, NewConversation};
use rim_core::conversation::repository::{
    ChannelListFilter, ConversationActivityCursor, ConversationCreatedCursor,
    ConversationRepository,
};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

#[tokio::test]
async fn create_conversation_and_get_by_id() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let conv = repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: Some("Team".to_string()),
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(conv.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.title.as_deref(), Some("Team"));
}

#[tokio::test]
async fn create_conversation_with_members_is_atomic() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u1 = ctx.create_user("atomic-c1").await.id;
    let u2 = ctx.create_user("atomic-c2").await.id;

    let conv = repo
        .create_conversation_with_members(
            NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: Some("Project".to_string()),
                is_private: false,
                created_by: Some(u1),
            },
            &[u1, u2],
        )
        .await
        .unwrap();

    let member_ids = member_repo.list_member_ids(conv.id).await.unwrap();
    assert_eq!(member_ids.len(), 2);
    assert!(member_ids.contains(&u1));
    assert!(member_ids.contains(&u2));
    assert_eq!(conv.title.as_deref(), Some("Project"));
    assert_eq!(conv.created_by, Some(u1));
    assert!(!conv.is_private);
    assert_eq!(conv.archived_at, None);
}

#[tokio::test]
async fn find_direct_between_users() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u1 = ctx.create_user("u1").await.id;
    let u2 = ctx.create_user("u2").await.id;

    assert!(
        conv_repo
            .find_direct_between_users(server.id, u1, u2)
            .await
            .unwrap()
            .is_none()
    );

    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Direct,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let _ = member_repo.add_member(conv.id, u1).await.unwrap();
    let _ = member_repo.add_member(conv.id, u2).await.unwrap();

    let found = conv_repo
        .find_direct_between_users(server.id, u1, u2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, conv.id);
}

#[tokio::test]
async fn touch_last_activity_at_updates_and_not_found() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let conv = repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let at = chrono::Utc::now() + chrono::Duration::seconds(10);
    repo.touch_last_activity_at(conv.id, at).await.unwrap();
    let fetched = repo.get_by_id(conv.id).await.unwrap().unwrap();
    assert!(fetched.last_activity_at.timestamp() >= at.timestamp());

    let err = repo
        .touch_last_activity_at(rim_core::conversation::model::ConversationId::new(), at)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn list_inbox_conversations_paginates_and_orders() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u = ctx.create_user("u").await.id;

    // Create 3 inbox conversations for user.
    let c1 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let c2 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let c3 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let _ = member_repo.add_member(c1.id, u).await.unwrap();
    let _ = member_repo.add_member(c2.id, u).await.unwrap();
    let _ = member_repo.add_member(c3.id, u).await.unwrap();

    // Touch last_activity_at so ordering is deterministic.
    let t1 = chrono::Utc::now() + chrono::Duration::seconds(1);
    let t2 = chrono::Utc::now() + chrono::Duration::seconds(2);
    let t3 = chrono::Utc::now() + chrono::Duration::seconds(3);
    conv_repo.touch_last_activity_at(c1.id, t1).await.unwrap();
    conv_repo.touch_last_activity_at(c2.id, t2).await.unwrap();
    conv_repo.touch_last_activity_at(c3.id, t3).await.unwrap();

    let page1 = conv_repo
        .list_inbox_conversations_for_user(u, server.id, 2, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next.is_some());

    // Next page using cursor.
    let page2 = conv_repo
        .list_inbox_conversations_for_user(u, server.id, 2, PageDir::After, page1.next)
        .await
        .unwrap();
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.items[0].id, c1.id);

    // Order should be last_activity_at DESC.
    assert!(page1.items[0].last_activity_at >= page1.items[1].last_activity_at);

    // Before should go the other way relative to cursor.
    let cursor = ConversationActivityCursor {
        last_activity_at: page1.items[1].last_activity_at,
        conversation_id: page1.items[1].id,
    };
    let before = conv_repo
        .list_inbox_conversations_for_user(u, server.id, 10, PageDir::Before, Some(cursor))
        .await
        .unwrap();
    assert!(!before.items.is_empty());
    assert_eq!(before.items[0].id, c3.id);
}

#[tokio::test]
async fn list_channels_for_user_paginates_and_orders_oldest_first() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u = ctx.create_user("u").await.id;

    let c1 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Channel,
            title: Some("alpha".to_string()),
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let c2 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Channel,
            title: Some("beta".to_string()),
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let _ = member_repo.add_member(c1.id, u).await.unwrap();
    let _ = member_repo.add_member(c2.id, u).await.unwrap();

    let page = conv_repo
        .list_channels_for_user(
            u,
            server.id,
            ChannelListFilter::default(),
            10,
            PageDir::After,
            None,
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].title.as_deref(), Some("alpha"));
    // ASC ordering by created_at.
    assert!(page.items[0].created_at <= page.items[1].created_at);

    let cursor = ConversationCreatedCursor {
        created_at: page.items[0].created_at,
        conversation_id: page.items[0].id,
    };
    let after = conv_repo
        .list_channels_for_user(
            u,
            server.id,
            ChannelListFilter::default(),
            10,
            PageDir::After,
            Some(cursor),
        )
        .await
        .unwrap();
    assert_eq!(after.items.len(), 1);
}

/// `Before` pages hold the inbox conversations adjacent to the cursor (finding A-H1). Endpoint
/// order is `last_activity_at DESC`, so `Before` walks toward more recently active ones.
#[tokio::test]
async fn list_inbox_before_returns_conversations_adjacent_to_cursor() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u = ctx.create_user("u").await.id;

    // convs[i] is the i-th least recently active.
    let base = chrono::Utc::now();
    let mut convs = Vec::new();
    for i in 0..5 {
        let c = conv_repo
            .create_conversation(NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: None,
                is_private: false,
                created_by: None,
            })
            .await
            .unwrap();
        member_repo.add_member(c.id, u).await.unwrap();
        conv_repo
            .touch_last_activity_at(c.id, base + chrono::Duration::seconds(i))
            .await
            .unwrap();
        convs.push(conv_repo.get_by_id(c.id).await.unwrap().unwrap());
    }
    let cursor_of = |i: usize| ConversationActivityCursor {
        last_activity_at: convs[i].last_activity_at,
        conversation_id: convs[i].id,
    };
    let ids = |idx: &[usize]| idx.iter().map(|&i| convs[i].id).collect::<Vec<_>>();

    let page1 = conv_repo
        .list_inbox_conversations_for_user(u, server.id, 2, PageDir::Before, Some(cursor_of(0)))
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        ids(&[2, 1])
    );
    assert_eq!(page1.next, Some(cursor_of(2)));
    assert_eq!(page1.prev, Some(cursor_of(1)));

    let page2 = conv_repo
        .list_inbox_conversations_for_user(u, server.id, 2, PageDir::Before, page1.next)
        .await
        .unwrap();
    assert_eq!(
        page2.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        ids(&[4, 3])
    );
    assert_eq!(page2.next, None);
    assert_eq!(page2.prev, Some(cursor_of(3)));
}

/// `Before` pages hold the channels adjacent to the cursor (finding A-H1). Endpoint order is
/// `created_at ASC`, so `Before` walks toward older channels.
#[tokio::test]
async fn list_channels_before_returns_channels_adjacent_to_cursor() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u = ctx.create_user("u").await.id;

    let mut channels = Vec::new();
    for i in 0..5 {
        let c = conv_repo
            .create_conversation(NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Channel,
                title: Some(format!("channel-{i}")),
                is_private: false,
                created_by: None,
            })
            .await
            .unwrap();
        channels.push(c);
    }
    // channels[i] is the i-th oldest by the endpoint's (created_at, id) key.
    channels.sort_by_key(|c| (c.created_at, uuid::Uuid::from(c.id)));
    let cursor_of = |i: usize| ConversationCreatedCursor {
        created_at: channels[i].created_at,
        conversation_id: channels[i].id,
    };
    let ids = |idx: &[usize]| idx.iter().map(|&i| channels[i].id).collect::<Vec<_>>();

    let page1 = conv_repo
        .list_channels_for_user(
            u,
            server.id,
            ChannelListFilter::default(),
            2,
            PageDir::Before,
            Some(cursor_of(4)),
        )
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        ids(&[2, 3])
    );
    assert_eq!(page1.next, Some(cursor_of(2)));
    assert_eq!(page1.prev, Some(cursor_of(3)));

    let page2 = conv_repo
        .list_channels_for_user(
            u,
            server.id,
            ChannelListFilter::default(),
            2,
            PageDir::Before,
            page1.next,
        )
        .await
        .unwrap();
    assert_eq!(
        page2.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        ids(&[0, 1])
    );
    assert_eq!(page2.next, None);
    assert_eq!(page2.prev, Some(cursor_of(1)));
}

// ── Track 1.1: private channels, joined filter, archive, accessible ids, back-fill ──

async fn channel(
    repo: &SeaOrmConversationRepository,
    server_id: ServerId,
    title: &str,
    is_private: bool,
    created_by: Option<UserId>,
) -> rim_core::conversation::model::Conversation {
    repo.create_conversation(NewConversation {
        server_id,
        conversation_type: ConversationType::Channel,
        title: Some(title.to_string()),
        is_private,
        created_by,
    })
    .await
    .unwrap()
}

fn titles(page: &[rim_core::conversation::model::Conversation]) -> Vec<String> {
    page.iter()
        .map(|c| c.title.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn create_persists_privacy_and_creator() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("priv-alice").await.id;

    let c = channel(&repo, server.id, "secret", true, Some(alice)).await;
    let fetched = repo.get_by_id(c.id).await.unwrap().unwrap();
    assert!(fetched.is_private);
    assert_eq!(fetched.created_by, Some(alice));
    assert_eq!(fetched.archived_at, None);
}

#[tokio::test]
async fn list_channels_hides_private_channels_from_non_members() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("lc-alice").await.id;
    let carol = ctx.create_user("lc-carol").await.id;

    let public = channel(&repo, server.id, "a-public", false, None).await;
    let private = channel(&repo, server.id, "b-private", true, Some(alice)).await;
    member_repo.add_member(public.id, alice).await.unwrap();
    member_repo.add_member(private.id, alice).await.unwrap();

    let all = ChannelListFilter::default();
    let joined = ChannelListFilter {
        joined_only: true,
        include_archived: false,
    };
    let list = |user, filter| {
        let repo = &repo;
        let server_id = server.id;
        async move {
            repo.list_channels_for_user(user, server_id, filter, 10, PageDir::After, None)
                .await
                .unwrap()
                .items
        }
    };

    assert_eq!(titles(&list(alice, all).await), ["a-public", "b-private"]);
    assert_eq!(
        titles(&list(alice, joined).await),
        ["a-public", "b-private"]
    );
    // carol sees the public channel in browse, nothing as joined, and never the private one.
    assert_eq!(titles(&list(carol, all).await), ["a-public"]);
    assert!(list(carol, joined).await.is_empty());

    member_repo.add_member(public.id, carol).await.unwrap();
    assert_eq!(titles(&list(carol, joined).await), ["a-public"]);
}

#[tokio::test]
async fn list_channels_excludes_archived_unless_asked() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("arch-alice").await.id;

    let live = channel(&repo, server.id, "a-live", false, None).await;
    let old = channel(&repo, server.id, "b-old", false, None).await;
    let old_private = channel(&repo, server.id, "c-old-private", true, None).await;
    member_repo.add_member(live.id, alice).await.unwrap();
    member_repo.add_member(old_private.id, alice).await.unwrap();

    let archived = repo
        .set_archived(old.id, Some(chrono::Utc::now()))
        .await
        .unwrap();
    assert!(archived.archived_at.is_some());
    repo.set_archived(old_private.id, Some(chrono::Utc::now()))
        .await
        .unwrap();

    let page = |filter| {
        let repo = &repo;
        let server_id = server.id;
        async move {
            repo.list_channels_for_user(alice, server_id, filter, 10, PageDir::After, None)
                .await
                .unwrap()
                .items
        }
    };
    assert_eq!(
        titles(&page(ChannelListFilter::default()).await),
        ["a-live"]
    );
    assert_eq!(
        titles(
            &page(ChannelListFilter {
                joined_only: false,
                include_archived: true,
            })
            .await
        ),
        ["a-live", "b-old", "c-old-private"]
    );
    assert_eq!(
        titles(
            &page(ChannelListFilter {
                joined_only: true,
                include_archived: true,
            })
            .await
        ),
        ["a-live", "c-old-private"]
    );

    let restored = repo.set_archived(old.id, None).await.unwrap();
    assert_eq!(restored.archived_at, None);
    assert_eq!(
        titles(&page(ChannelListFilter::default()).await),
        ["a-live", "b-old"]
    );

    let err = repo
        .set_archived(ConversationId::new(), None)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::NotFound);
}

#[tokio::test]
async fn accessible_ids_include_private_channels_only_for_members() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("acc-alice").await.id;
    let carol = ctx.create_user("acc-carol").await.id;

    let public = channel(&repo, server.id, "public", false, None).await;
    let archived_public = channel(&repo, server.id, "archived", false, None).await;
    repo.set_archived(archived_public.id, Some(chrono::Utc::now()))
        .await
        .unwrap();
    let private = channel(&repo, server.id, "private", true, None).await;
    member_repo.add_member(private.id, alice).await.unwrap();
    let group = repo
        .create_conversation_with_members(
            NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: None,
                is_private: false,
                created_by: Some(alice),
            },
            &[alice],
        )
        .await
        .unwrap();

    let mut alice_ids = repo
        .list_accessible_conversation_ids(server.id, alice)
        .await
        .unwrap();
    alice_ids.sort_by_key(|id| uuid::Uuid::from(*id));
    let mut expected = vec![public.id, archived_public.id, private.id, group.id];
    expected.sort_by_key(|id| uuid::Uuid::from(*id));
    assert_eq!(alice_ids, expected);

    let mut carol_ids = repo
        .list_accessible_conversation_ids(server.id, carol)
        .await
        .unwrap();
    carol_ids.sort_by_key(|id| uuid::Uuid::from(*id));
    let mut expected = vec![public.id, archived_public.id];
    expected.sort_by_key(|id| uuid::Uuid::from(*id));
    assert_eq!(carol_ids, expected);
}

/// The Track 1.1 migration data step: every workspace member becomes a member of every channel
/// of their workspace; existing rows, other workspaces and non-channels are untouched.
#[tokio::test]
async fn migration_backfill_makes_workspace_members_channel_members() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server_member_repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let other_server = ctx.create_server("other").await;
    let alice = ctx.create_user("bf-alice").await.id;
    let bob = ctx.create_user("bf-bob").await.id;
    let outsider = ctx.create_user("bf-outsider").await.id;
    server_member_repo
        .add_member(server.id, alice, ServerMemberRole::Owner)
        .await
        .unwrap();
    server_member_repo
        .add_member(server.id, bob, ServerMemberRole::Guest)
        .await
        .unwrap();
    server_member_repo
        .add_member(other_server.id, outsider, ServerMemberRole::Owner)
        .await
        .unwrap();

    let general = channel(&repo, server.id, "general", false, Some(alice)).await;
    let alice_row = member_repo.add_member(general.id, alice).await.unwrap();
    let group = repo
        .create_conversation_with_members(
            NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: None,
                is_private: false,
                created_by: Some(alice),
            },
            &[alice],
        )
        .await
        .unwrap();

    ctx.db
        .execute_unprepared(migration::CHANNEL_MEMBERSHIP_BACKFILL_SQL)
        .await
        .unwrap();
    // Idempotent: a second run inserts nothing.
    ctx.db
        .execute_unprepared(migration::CHANNEL_MEMBERSHIP_BACKFILL_SQL)
        .await
        .unwrap();

    let members = member_repo
        .get_members_for_conversation(general.id)
        .await
        .unwrap();
    let mut ids: Vec<_> = members.iter().map(|m| m.user_id).collect();
    ids.sort_by_key(|id| uuid::Uuid::from(*id));
    let mut expected = vec![alice, bob];
    expected.sort_by_key(|id| uuid::Uuid::from(*id));
    assert_eq!(
        ids, expected,
        "outsider from another workspace must not be added"
    );
    assert!(
        members.iter().any(|m| m.id == alice_row.id),
        "existing membership row is kept"
    );
    let bob_row = members.iter().find(|m| m.user_id == bob).unwrap();
    assert_eq!(
        uuid::Uuid::from(bob_row.id).get_version_num(),
        7,
        "back-filled ids are UUIDv7"
    );
    assert_eq!(
        member_repo.list_member_ids(group.id).await.unwrap(),
        vec![alice]
    );
}

fn sorted(mut ids: Vec<ConversationId>) -> Vec<ConversationId> {
    ids.sort_by_key(|id| uuid::Uuid::from(*id));
    ids
}

/// The auto-join set: public, unarchived channels of this server only.
#[tokio::test]
async fn public_channel_ids_exclude_private_archived_and_other_servers() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let other = ctx.create_server("other").await;

    let public = channel(&repo, server.id, "public", false, None).await;
    channel(&repo, server.id, "private", true, None).await;
    let archived = channel(&repo, server.id, "archived", false, None).await;
    repo.set_archived(archived.id, Some(chrono::Utc::now()))
        .await
        .unwrap();
    channel(&repo, other.id, "elsewhere", false, None).await;
    repo.create_conversation(NewConversation {
        server_id: server.id,
        conversation_type: ConversationType::Group,
        title: None,
        is_private: false,
        created_by: None,
    })
    .await
    .unwrap();

    assert_eq!(
        repo.list_public_channel_ids(server.id).await.unwrap(),
        vec![public.id]
    );
}

/// The unread-count scope: only conversations with a membership row, in this server.
#[tokio::test]
async fn joined_conversation_ids_cover_membership_rows_only() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let other = ctx.create_server("other").await;
    let alice = ctx.create_user("joined-alice").await.id;

    let joined_public = channel(&repo, server.id, "joined", false, None).await;
    channel(&repo, server.id, "unjoined", false, None).await;
    let private = channel(&repo, server.id, "private", true, None).await;
    let elsewhere = channel(&repo, other.id, "elsewhere", false, None).await;
    for id in [joined_public.id, private.id, elsewhere.id] {
        member_repo.add_member(id, alice).await.unwrap();
    }
    let group = repo
        .create_conversation_with_members(
            NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: None,
                is_private: false,
                created_by: Some(alice),
            },
            &[alice],
        )
        .await
        .unwrap();

    assert_eq!(
        sorted(
            repo.list_joined_conversation_ids(server.id, alice)
                .await
                .unwrap()
        ),
        sorted(vec![joined_public.id, private.id, group.id])
    );
}
