#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;

#[tokio::test]
async fn add_get_is_member_list_member_ids_and_get_members() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let u1 = ctx.create_user("u1").await.id;
    let u2 = ctx.create_user("u2").await.id;

    let cm1 = repo.add_member(conv.id, u1).await.unwrap();
    let _cm2 = repo.add_member(conv.id, u2).await.unwrap();

    let by_id = repo.get_by_id(cm1.id).await.unwrap().unwrap();
    assert_eq!(by_id.user_id, u1);

    assert!(repo.is_member(conv.id, u1).await.unwrap());
    assert!(!repo.is_member(conv.id, UserId::new()).await.unwrap());

    let ids = repo.list_member_ids(conv.id).await.unwrap();
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn remove_member_success_and_not_found() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let u = ctx.create_user("u").await.id;
    let err = repo.remove_member(conv.id, u).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let _ = repo.add_member(conv.id, u).await.unwrap();
    repo.remove_member(conv.id, u).await.unwrap();
    assert!(!repo.is_member(conv.id, u).await.unwrap());
}

#[tokio::test]
async fn add_member_conflicts_on_duplicate_conversation_membership() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();
    let user_id = ctx.create_user("dup-conv-member").await.id;

    repo.add_member(conv.id, user_id).await.unwrap();

    let err = repo.add_member(conv.id, user_id).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}

#[tokio::test]
async fn list_member_ids_for_conversations_batch() {
    let ctx = support::TestCtx::new().await;

    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;

    let conv1 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let conv2 = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
            is_private: false,
            created_by: None,
        })
        .await
        .unwrap();

    let u1 = ctx.create_user("batch-u1").await.id;
    let u2 = ctx.create_user("batch-u2").await.id;
    let u3 = ctx.create_user("batch-u3").await.id;

    // conv1 has u1, u2
    repo.add_member(conv1.id, u1).await.unwrap();
    repo.add_member(conv1.id, u2).await.unwrap();

    // conv2 has u2, u3
    repo.add_member(conv2.id, u2).await.unwrap();
    repo.add_member(conv2.id, u3).await.unwrap();

    let map = repo
        .list_member_ids_for_conversations(&[conv1.id, conv2.id])
        .await
        .unwrap();

    let conv1_members = map.get(&conv1.id).expect("conv1 should be in map");
    assert_eq!(conv1_members.len(), 2);
    assert!(conv1_members.contains(&u1));
    assert!(conv1_members.contains(&u2));

    let conv2_members = map.get(&conv2.id).expect("conv2 should be in map");
    assert_eq!(conv2_members.len(), 2);
    assert!(conv2_members.contains(&u2));
    assert!(conv2_members.contains(&u3));
}
