#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn add_get_is_member_list_member_ids_and_get_members() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let repo = SeaOrmConversationMemberRepository::new(db.clone());
    let user_repo = SeaOrmUserRepository::new(db.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();

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

    let cm1 = repo.add_member(conv.id, u1).await.unwrap();
    let _cm2 = repo.add_member(conv.id, u2).await.unwrap();

    let by_id = repo.get_by_id(cm1.id).await.unwrap().unwrap();
    assert_eq!(by_id.user_id, u1);

    assert_eq!(repo.is_member(conv.id, u1).await.unwrap(), true);
    assert_eq!(repo.is_member(conv.id, UserId::new()).await.unwrap(), false);

    let ids = repo.list_member_ids(conv.id).await.unwrap();
    assert_eq!(ids.len(), 2);

    let members = repo.get_members_for_conversation(conv.id).await.unwrap();
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn remove_member_success_and_not_found() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let repo = SeaOrmConversationMemberRepository::new(db.clone());
    let user_repo = SeaOrmUserRepository::new(db.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();

    let u = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;
    let err = repo.remove_member(conv.id, u).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let _ = repo.add_member(conv.id, u).await.unwrap();
    repo.remove_member(conv.id, u).await.unwrap();
    assert_eq!(repo.is_member(conv.id, u).await.unwrap(), false);
}

#[tokio::test]
async fn add_member_conflicts_on_duplicate_conversation_membership() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let repo = SeaOrmConversationMemberRepository::new(db.clone());
    let user_repo = SeaOrmUserRepository::new(db.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();
    let user_id = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "dup-conv-member".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    repo.add_member(conv.id, user_id).await.unwrap();

    let err = repo.add_member(conv.id, user_id).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}
