//! The counts the instance limits read: workspaces a user owns, members and channels of a
//! workspace, its macros and pipelines, and the invites a user created since a time.
#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};

use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::InviteRepository;
use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::macro_def::model::NewMacroDef;
use rim_core::macro_def::repository::MacroRepository;
use rim_core::pipeline::model::{
    Pipeline, PipelineAction, PipelineId, PipelineTrigger, PipelineTriggerKind,
};
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::macro_def::SeaOrmMacroRepository;
use rim_infra_db::sea_orm_impl::pipeline::SeaOrmPipelineRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

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
    ctx.create_conversation(other.id, ConversationType::Channel)
        .await;
    assert_eq!(conversations.count_channels(owned.id).await.unwrap(), 2);

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
