//! The counts the instance limits read: workspaces a user owns, members and channels (unarchived
//! and all) of a workspace, its macros and pipelines, the invites a user created since a time,
//! and the local accounts with the lock signups count them under.
#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};

use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::InviteRepository;
use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::host::model::NewRemoteUser;
use rim_core::host::repository::HostRepository;
use rim_core::macro_def::model::NewMacroDef;
use rim_core::macro_def::repository::MacroRepository;
use rim_core::pipeline::model::{
    Pipeline, PipelineAction, PipelineId, PipelineTrigger, PipelineTriggerKind,
};
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{NewUser, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::begin_transaction;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::macro_def::SeaOrmMacroRepository;
use rim_infra_db::sea_orm_impl::pipeline::SeaOrmPipelineRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

fn pipeline(server_id: rim_core::server::model::ServerId, created_by: UserId) -> Pipeline {
    let now = Utc::now();
    Pipeline {
        id: PipelineId::new(),
        server_id,
        name: "p".to_string(),
        description: None,
        trigger: PipelineTrigger {
            kind: PipelineTriggerKind::NewMessage,
            conversation_id: None,
        },
        filter: None,
        actions: vec![PipelineAction::ExecuteCommand {
            command_id: "chat.post_message".to_string(),
            args: None,
        }],
        enabled: true,
        cooldown_secs: None,
        created_by,
        schema_version: 1,
        last_fired_at: None,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn each_count_covers_its_workspace_or_user_alone() {
    let ctx = support::TestCtx::new().await;
    let servers = SeaOrmServerRepository::new(ctx.db.clone());
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let conversations = SeaOrmConversationRepository::new(ctx.db.clone());
    let macros = SeaOrmMacroRepository::new(ctx.db.clone());
    let pipelines = SeaOrmPipelineRepository::new(ctx.db.clone());
    let invites = SeaOrmInviteRepository::new(ctx.db.clone());

    let (ada, bob) = (ctx.create_user("ada").await, ctx.create_user("bob").await);
    let (owned, other) = (
        servers.create_server("owned").await.unwrap(),
        servers.create_server("other").await.unwrap(),
    );
    members
        .add_member(owned.id, ada.id, ServerMemberRole::Owner)
        .await
        .unwrap();
    members
        .add_member(owned.id, bob.id, ServerMemberRole::Guest)
        .await
        .unwrap();
    members
        .add_member(other.id, ada.id, ServerMemberRole::Member)
        .await
        .unwrap();
    assert_eq!(servers.count_owned(ada.id).await.unwrap(), 1);
    assert_eq!(servers.count_owned(bob.id).await.unwrap(), 0);
    assert_eq!(members.count_members(owned.id).await.unwrap(), 2);
    assert_eq!(members.count_members(other.id).await.unwrap(), 1);

    for kind in [
        ConversationType::Channel,
        ConversationType::Channel,
        ConversationType::Group,
    ] {
        ctx.create_conversation(owned.id, kind).await;
    }
    let archived = ctx
        .create_conversation(owned.id, ConversationType::Channel)
        .await;
    conversations
        .set_archived(archived.id, Some(Utc::now()))
        .await
        .unwrap();
    ctx.create_conversation(other.id, ConversationType::Channel)
        .await;
    assert_eq!(conversations.count_channels(owned.id).await.unwrap(), 2);
    assert_eq!(conversations.count_all_channels(owned.id).await.unwrap(), 3);

    for (user, name) in [(ada.id, "a"), (bob.id, "b")] {
        macros
            .create(NewMacroDef {
                server_id: owned.id,
                user_id: user,
                name: name.to_string(),
                description: None,
                steps_json: serde_json::json!([]),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
            })
            .await
            .unwrap();
        pipelines.create(&pipeline(owned.id, user)).await.unwrap();
    }
    assert_eq!(macros.count_for_server(owned.id).await.unwrap(), 2);
    assert_eq!(macros.count_for_server(other.id).await.unwrap(), 0);
    assert_eq!(pipelines.count_for_server(owned.id).await.unwrap(), 2);
    assert_eq!(pipelines.count_for_server(other.id).await.unwrap(), 0);

    let since = Utc::now() - Duration::seconds(5);
    for (by, n) in [(ada.id, 0), (ada.id, 1), (bob.id, 2)] {
        invites
            .create_invite(NewInvite {
                server_id: owned.id,
                created_by_user_id: by,
                token_hash: format!("count-{n}"),
                expires_at: None,
                max_uses: 1,
                role: ServerMemberRole::Member,
            })
            .await
            .unwrap();
    }
    assert_eq!(invites.count_created_since(ada.id, since).await.unwrap(), 2);
    assert_eq!(invites.count_created_since(bob.id, since).await.unwrap(), 1);
    assert_eq!(
        invites
            .count_created_since(ada.id, Utc::now() + Duration::seconds(5))
            .await
            .unwrap(),
        0
    );
}

/// Run `sql` with the user id `id` as `$1`.
async fn update_user(db: &rim_infra_db::DbHandle, sql: &str, id: UserId) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [uuid::Uuid::from(id).into()],
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn local_accounts_count_those_in_their_grace_period_but_no_remote_user_or_tombstone() {
    let ctx = support::TestCtx::new().await;
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    assert_eq!(users.count_local_accounts(10).await.unwrap(), 0);

    ctx.create_user("ada").await;
    let leaving = ctx.create_user("leaving").await;
    let gone = ctx.create_user("gone").await;
    SeaOrmHostRepository::new(ctx.db.clone())
        .create_remote_user(NewRemoteUser {
            principal_id: "rimp1_remote".to_string(),
            identity_key: "ed25519:remote".to_string(),
            username: "r_remote".to_string(),
            display_name: "Remote".to_string(),
            home_relay: Some("https://relay.example".to_string()),
        })
        .await
        .unwrap();
    update_user(
        &ctx.db,
        r#"UPDATE "user" SET deletion_requested_at = now(),
               deletion_scheduled_for = now() + interval '14 days' WHERE id = $1"#,
        leaving.id,
    )
    .await;
    update_user(
        &ctx.db,
        r#"UPDATE "user" SET deleted_at = now(), username = 'deleted-0000gone', email = NULL,
               password_hash = NULL, principal_id = 'rimp1_gone' WHERE id = $1"#,
        gone.id,
    )
    .await;

    assert_eq!(users.count_local_accounts(10).await.unwrap(), 2);
    assert_eq!(
        users.count_local_accounts(1).await.unwrap(),
        1,
        "counted up to at_most"
    );
    assert_eq!(users.count_local_accounts(0).await.unwrap(), 0);
}

#[tokio::test]
async fn signups_counting_under_the_lock_never_pass_the_cap() {
    const CAP: u64 = 3;
    let _ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let mut tasks = Vec::new();
    for n in 0..12 {
        let conn = conn.clone();
        tasks.push(tokio::spawn(async move {
            let scope = begin_transaction(&conn).await.unwrap();
            let users = SeaOrmUserRepository::new(scope.handle());
            users.lock_account_creation().await.unwrap();
            if users.count_local_accounts(CAP).await.unwrap() >= CAP {
                return false;
            }
            users
                .create_user(NewUser {
                    username: format!("racer{n}"),
                    password: Some("test_hash".to_string()),
                    email: Some(format!("racer{n}@test.example.com")),
                    display_name: None,
                    key_identity: None,
                })
                .await
                .unwrap();
            scope.commit().await.unwrap();
            true
        }));
    }
    let mut admitted = 0;
    for task in tasks {
        admitted += u64::from(task.await.unwrap());
    }
    assert_eq!(admitted, CAP);
    let users = SeaOrmUserRepository::new(conn.into());
    assert_eq!(users.count_local_accounts(100).await.unwrap(), CAP);
}
