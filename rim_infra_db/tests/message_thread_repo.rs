#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::{MessageId, NewMessage};
use rim_core::message::repository::MessageRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::NewUser;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_get_and_list_threads_for_conversation() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let repo = SeaOrmMessageThreadRepository::new(db.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();

    let t1 = repo.create_thread(conv.id).await.unwrap();
    let t2 = repo.create_thread(conv.id).await.unwrap();

    let at1 = chrono::Utc::now() + chrono::Duration::seconds(1);
    let at2 = chrono::Utc::now() + chrono::Duration::seconds(2);
    repo.touch_last_message_at(t1.id, at1).await.unwrap();
    repo.touch_last_message_at(t2.id, at2).await.unwrap();

    assert!(repo.get_by_id(t1.id).await.unwrap().is_some());
    assert!(repo.get_by_id(t2.id).await.unwrap().is_some());

    let page = repo
        .list_threads_for_conversation(conv.id, 10, PageDir::After, None)
        .await
        .unwrap();
    assert!(page.items.len() >= 2);

    let page1 = repo
        .list_threads_for_conversation(conv.id, 2, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next.is_none());
    assert!(page1.prev.is_none());

    let page2 = repo
        .list_threads_for_conversation(
            conv.id,
            1,
            PageDir::After,
            Some(rim_core::message_thread::repository::ThreadCursor {
                last_message_at: page1.items[1].last_message_at,
                thread_id: page1.items[1].id,
            }),
        )
        .await
        .unwrap();
    assert!(page2.items.is_empty());

    let before = repo
        .list_threads_for_conversation(
            conv.id,
            10,
            PageDir::Before,
            Some(rim_core::message_thread::repository::ThreadCursor {
                last_message_at: page1.items[1].last_message_at,
                thread_id: page1.items[1].id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].id, page1.items[0].id);
}

#[tokio::test]
async fn set_first_message_conflict_and_not_found() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(db.clone());
    let msg_repo = SeaOrmMessageRepository::new(db.clone());

    let user = user_repo
        .create_user(NewUser {
            username: "testuser".to_string(),
            password: "hash".to_string(),
        })
        .await
        .unwrap();
    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();

    let msg = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
            text: Some("t".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();

    thread_repo
        .set_first_message(thread.id, msg.id)
        .await
        .unwrap();

    let fetched = thread_repo
        .get_by_root_message(msg.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.id, thread.id);

    let other_thread = thread_repo.create_thread(conv.id).await.unwrap();
    let err = thread_repo
        .set_first_message(other_thread.id, msg.id)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));

    let err = thread_repo
        .set_first_message(thread.id, MessageId::new())
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));

    let err = thread_repo
        .set_first_message(
            rim_core::message_thread::model::MessageThreadId::new(),
            msg.id,
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn create_thread_for_root_is_atomic_and_conflicts_on_duplicate_root() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(db.clone());
    let msg_repo = SeaOrmMessageRepository::new(db.clone());

    let user = user_repo
        .create_user(NewUser {
            username: "testuser".to_string(),
            password: "hash".to_string(),
        })
        .await
        .unwrap();
    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();
    let msg = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: None,
            parent_message_id: None,
            sent_by: user.id,
            text: Some("root".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();

    let thread = thread_repo
        .create_thread_for_root(conv.id, msg.id)
        .await
        .unwrap();
    assert_eq!(thread.first_message_id, Some(msg.id));

    let err = thread_repo
        .create_thread_for_root(conv.id, msg.id)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}

#[tokio::test]
async fn touch_last_message_at_not_found() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let repo = SeaOrmMessageThreadRepository::new(db.clone());
    let err = repo
        .touch_last_message_at(
            rim_core::message_thread::model::MessageThreadId::new(),
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}
