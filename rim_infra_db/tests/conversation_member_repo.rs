#![cfg(feature = "db-tests")]

mod support;

use std::collections::HashSet;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::model::Membership;
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
            encrypted: false,
        })
        .await
        .unwrap();

    let u1 = ctx.create_user("u1").await.id;
    let u2 = ctx.create_user("u2").await.id;

    repo.add_member(conv.id, u1).await.unwrap();
    repo.add_member(conv.id, u2).await.unwrap();

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
            encrypted: false,
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
            encrypted: false,
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
            encrypted: false,
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
            encrypted: false,
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

/// Pending members are kept apart: `is_member` and the member lists see active members only.
#[tokio::test]
async fn pending_members_are_listed_apart_from_active_ones() {
    let ctx = support::TestCtx::new().await;
    let conv_repo = SeaOrmConversationRepository::new(ctx.db.clone());
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("pm-alice").await.id;
    let bob = ctx.create_user("pm-bob").await.id;
    let carol = ctx.create_user("pm-carol").await.id;
    let conv = conv_repo
        .create_conversation_with_members(
            NewConversation {
                server_id: server.id,
                conversation_type: ConversationType::Group,
                title: None,
                is_private: false,
                created_by: Some(alice),
                encrypted: true,
            },
            &[alice],
            &[bob],
        )
        .await
        .unwrap();
    let pending = repo.add_pending_member(conv.id, carol).await.unwrap();
    assert!(pending.pending_since.is_some());
    let err = repo.add_member(conv.id, carol).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));

    assert!(repo.is_member(conv.id, alice).await.unwrap());
    assert!(!repo.is_member(conv.id, bob).await.unwrap());
    let membership = |u| repo.membership(conv.id, u);
    assert_eq!(membership(alice).await.unwrap(), Some(Membership::Active));
    assert_eq!(membership(bob).await.unwrap(), Some(Membership::Pending));
    assert_eq!(membership(UserId::new()).await.unwrap(), None);

    assert_eq!(repo.list_member_ids(conv.id).await.unwrap(), vec![alice]);
    let waiting: HashSet<UserId> = repo
        .list_pending_member_ids(conv.id)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(waiting, HashSet::from([bob, carol]));
    let active = repo
        .list_member_ids_for_conversations(&[conv.id])
        .await
        .unwrap();
    assert_eq!(active[&conv.id], vec![alice]);
    let batch = repo
        .list_pending_member_ids_for_conversations(&[conv.id])
        .await
        .unwrap();
    assert_eq!(batch[&conv.id].len(), 2);
    assert_eq!(
        repo.list_pending_conversation_ids(bob).await.unwrap(),
        vec![conv.id]
    );
    assert!(
        repo.list_pending_conversation_ids(alice)
            .await
            .unwrap()
            .is_empty()
    );

    // A pending member is removed like any other.
    repo.remove_member(conv.id, carol).await.unwrap();
    assert_eq!(membership(carol).await.unwrap(), None);
}

/// Activation flips a pending member once and restarts their `joined_at`.
#[tokio::test]
async fn activate_member_flips_a_pending_member_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("act-alice").await.id;
    let bob = ctx.create_user("act-bob").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    repo.add_member(conv.id, alice).await.unwrap();
    let pending = repo.add_pending_member(conv.id, bob).await.unwrap();

    assert!(!repo.activate_member(conv.id, alice).await.unwrap());
    assert!(!repo.activate_member(conv.id, UserId::new()).await.unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(repo.activate_member(conv.id, bob).await.unwrap());
    assert!(!repo.activate_member(conv.id, bob).await.unwrap());

    assert!(repo.is_member(conv.id, bob).await.unwrap());
    assert!(
        repo.list_pending_conversation_ids(bob)
            .await
            .unwrap()
            .is_empty()
    );
    let members: HashSet<UserId> = repo
        .list_member_ids(conv.id)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(members, HashSet::from([alice, bob]));
    let joined_at: chrono::DateTime<chrono::Utc> = sea_orm::ConnectionTrait::query_one_raw(
        &ctx.db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT joined_at FROM conversation_member WHERE conversation_id = $1 AND user_id = $2",
            [
                sea_orm::prelude::Uuid::from(conv.id).into(),
                sea_orm::prelude::Uuid::from(bob).into(),
            ],
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .try_get("", "joined_at")
    .unwrap();
    assert!(joined_at > pending.joined_at);
}

/// The member who added someone is recorded on their membership, active or pending, read
/// back, and cleared when that account is deleted.
#[tokio::test]
async fn invited_by_is_recorded_per_membership() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let alice = ctx.create_user("inv-alice").await.id;
    let bob = ctx.create_user("inv-bob").await.id;
    let carol = ctx.create_user("inv-carol").await.id;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    repo.add_member(conv.id, alice).await.unwrap();
    repo.add_member(conv.id, bob).await.unwrap();
    repo.add_pending_member(conv.id, carol).await.unwrap();

    assert_eq!(repo.invited_by(conv.id, bob).await.unwrap(), None);
    assert!(repo.set_invited_by(conv.id, bob, alice).await.unwrap());
    assert!(repo.set_invited_by(conv.id, carol, bob).await.unwrap());
    assert!(
        !repo
            .set_invited_by(conv.id, UserId::new(), alice)
            .await
            .unwrap()
    );
    assert_eq!(repo.invited_by(conv.id, bob).await.unwrap(), Some(alice));
    assert_eq!(repo.invited_by(conv.id, carol).await.unwrap(), Some(bob));
    assert_eq!(repo.invited_by(conv.id, alice).await.unwrap(), None);
    assert_eq!(repo.invited_by(conv.id, UserId::new()).await.unwrap(), None);

    sea_orm::ConnectionTrait::execute_raw(
        &ctx.db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "DELETE FROM \"user\" WHERE id = $1",
            [sea_orm::prelude::Uuid::from(alice).into()],
        ),
    )
    .await
    .unwrap();
    assert_eq!(repo.invited_by(conv.id, bob).await.unwrap(), None);
    assert_eq!(repo.invited_by(conv.id, carol).await.unwrap(), Some(bob));
}

/// The expiry purge deletes only pending memberships that started before the cutoff.
#[tokio::test]
async fn delete_pending_before_removes_old_invitations_only() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmConversationMemberRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let member = ctx.create_user("exp-member").await.id;
    let old = ctx.create_user("exp-old").await.id;
    let fresh = ctx.create_user("exp-fresh").await.id;
    repo.add_member(conv.id, member).await.unwrap();
    repo.add_pending_member(conv.id, old).await.unwrap();
    repo.add_pending_member(conv.id, fresh).await.unwrap();
    sea_orm::ConnectionTrait::execute_raw(
        &ctx.db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "UPDATE conversation_member SET pending_since = now() - interval '31 days' \
             WHERE user_id = $1",
            [sea_orm::prelude::Uuid::from(old).into()],
        ),
    )
    .await
    .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    assert_eq!(repo.delete_pending_before(cutoff).await.unwrap(), 1);
    assert_eq!(repo.delete_pending_before(cutoff).await.unwrap(), 0);
    assert_eq!(repo.membership(conv.id, old).await.unwrap(), None);
    assert_eq!(
        repo.membership(conv.id, fresh).await.unwrap(),
        Some(Membership::Pending)
    );
    assert_eq!(
        repo.membership(conv.id, member).await.unwrap(),
        Some(Membership::Active)
    );
}
