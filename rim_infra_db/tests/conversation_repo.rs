#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::{
    ConversationActivityCursor, ConversationCreatedCursor, ConversationRepository,
};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_core::server::repository::ServerRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_conversation_and_get_by_id() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let repo = SeaOrmConversationRepository::new(conn.clone());
    let server = server_repo.create_server("s").await.unwrap();
    let conv = repo
        .create_conversation(ConversationType::Group, server.id, Some("Team".to_string()))
        .await
        .unwrap();

    let fetched = repo.get_by_id(conv.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.title.as_deref(), Some("Team"));
}

#[tokio::test]
async fn create_conversation_with_members_is_atomic() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let repo = SeaOrmConversationRepository::new(conn.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u1 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "atomic-c1".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;
    let u2 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "atomic-c2".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    let conv = repo
        .create_conversation_with_members(
            ConversationType::Group,
            server.id,
            Some("Project".to_string()),
            &[u1, u2],
        )
        .await
        .unwrap();

    let member_ids = member_repo.list_member_ids(conv.id).await.unwrap();
    assert_eq!(member_ids.len(), 2);
    assert!(member_ids.contains(&u1));
    assert!(member_ids.contains(&u2));
    assert_eq!(conv.title.as_deref(), Some("Project"));
}

#[tokio::test]
async fn find_direct_between_users() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u1 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u1".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;
    let u2 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u2".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    assert!(
        conv_repo
            .find_direct_between_users(server.id, u1, u2)
            .await
            .unwrap()
            .is_none()
    );

    let conv = conv_repo
        .create_conversation(ConversationType::Direct, server.id, None)
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
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let repo = SeaOrmConversationRepository::new(conn.clone());
    let _user_repo = SeaOrmUserRepository::new(conn.clone());
    let server = server_repo.create_server("s").await.unwrap();
    let conv = repo
        .create_conversation(ConversationType::Group, server.id, None)
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
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    // Create 3 inbox conversations for user.
    let c1 = conv_repo
        .create_conversation(ConversationType::Group, server.id, None)
        .await
        .unwrap();
    let c2 = conv_repo
        .create_conversation(ConversationType::Group, server.id, None)
        .await
        .unwrap();
    let c3 = conv_repo
        .create_conversation(ConversationType::Group, server.id, None)
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
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let member_repo = SeaOrmConversationMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    let c1 = conv_repo
        .create_conversation(
            ConversationType::Channel,
            server.id,
            Some("alpha".to_string()),
        )
        .await
        .unwrap();
    let c2 = conv_repo
        .create_conversation(
            ConversationType::Channel,
            server.id,
            Some("beta".to_string()),
        )
        .await
        .unwrap();
    let _ = member_repo.add_member(c1.id, u).await.unwrap();
    let _ = member_repo.add_member(c2.id, u).await.unwrap();

    let page = conv_repo
        .list_channels_for_user(u, server.id, 10, PageDir::After, None)
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
        .list_channels_for_user(u, server.id, 10, PageDir::After, Some(cursor))
        .await
        .unwrap();
    assert_eq!(after.items.len(), 1);
}
