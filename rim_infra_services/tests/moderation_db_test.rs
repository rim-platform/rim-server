//! Roles and moderation against a live database: a ban blocks re-joining through the auth
//! service, and a guest invite grants the restricted role without the public-channel auto-join.
//!
//! Only compiled with `--features infra-db-tests`; needs `DATABASE_URL` (see `support`).

#![cfg(feature = "infra-db-tests")]

mod support;

use std::sync::Arc;

use chrono::{Duration, Utc};
use rim_auth::session_token::hash_token;
use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::InviteRepository;
use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::moderation::model::ServerBan;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::moderation::SeaOrmModerationRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::AppRepos;
use rim_infra_services::auth::DefaultAuthService;
use rim_services::auth::command::JoinServerByInviteCmd;
use rim_services::auth::service::AuthService;
use rim_services::events::{NoopBroadcaster, Observers};
use rim_services::{Actor, ServiceError};

fn build_auth_service(db: sea_orm::DatabaseConnection) -> DefaultAuthService {
    let repos = AppRepos::with_handle(&DbHandle::Connection(db.clone())).for_auth();
    DefaultAuthService::new(
        repos,
        Some(db),
        Duration::days(30),
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>),
        Arc::new(rim_infra_services::email::LogEmailSender),
        None,
    )
}

async fn create_user(handle: &DbHandle, name: &str) -> rim_core::user::model::User {
    SeaOrmUserRepository::new(handle.clone())
        .create_user(rim_core::user::model::NewUser {
            username: name.to_string(),
            password: Some("hash".to_string()),
            email: Some(format!("{name}@example.com")),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("create user")
}

#[tokio::test]
async fn ban_blocks_rejoin_until_lifted() {
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    let handle = DbHandle::Connection(db.clone());

    let owner = create_user(&handle, "banowner").await;
    let bob = create_user(&handle, "banbob").await;
    let server = SeaOrmServerRepository::new(handle.clone())
        .create_server("ban-server")
        .await
        .expect("server");
    let members = SeaOrmServerMemberRepository::new(handle.clone());
    members
        .add_member(server.id, owner.id, ServerMemberRole::Owner)
        .await
        .expect("owner");

    let invites = SeaOrmInviteRepository::new(handle.clone());
    let raw_token = "rim_inv_ban_rejoin";
    let invite = invites
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: owner.id,
            token_hash: hash_token(raw_token),
            expires_at: None,
            max_uses: 5,
            role: ServerMemberRole::Member,
        })
        .await
        .expect("invite");

    let moderation = SeaOrmModerationRepository::new(handle.clone());
    moderation
        .upsert_ban(ServerBan {
            server_id: server.id,
            user_id: bob.id,
            reason: None,
            banned_by: Some(owner.id),
            created_at: Utc::now(),
            expires_at: None,
        })
        .await
        .expect("ban");

    let svc = build_auth_service(db.clone());
    let join = || {
        svc.join_server_by_invite(JoinServerByInviteCmd {
            actor: Actor::new(bob.id),
            invite_token: raw_token.to_string(),
        })
    };

    let err = join().await.unwrap_err();
    assert!(
        matches!(&err, ServiceError::Forbidden(message) if message == "you are banned from this workspace"),
        "{err:?}"
    );
    assert!(!members.is_member(server.id, bob.id).await.expect("lookup"));
    assert_eq!(
        invites
            .get_by_id(invite.id)
            .await
            .expect("invite")
            .expect("exists")
            .use_count,
        0,
        "a refused join consumes no invite use"
    );

    // An expired ban no longer blocks.
    moderation
        .upsert_ban(ServerBan {
            server_id: server.id,
            user_id: bob.id,
            reason: None,
            banned_by: Some(owner.id),
            created_at: Utc::now() - Duration::days(2),
            expires_at: Some(Utc::now() - Duration::days(1)),
        })
        .await
        .expect("expire ban");
    join().await.expect("join after the ban expired");
    assert!(members.is_member(server.id, bob.id).await.expect("lookup"));
}

#[tokio::test]
async fn guest_invite_grants_guest_without_channel_auto_join() {
    let db = support::db_conn().await;
    support::reset_db(&db).await;
    let handle = DbHandle::Connection(db.clone());

    let owner = create_user(&handle, "guestowner").await;
    let guest = create_user(&handle, "guestuser").await;
    let member = create_user(&handle, "memberuser").await;
    let server = SeaOrmServerRepository::new(handle.clone())
        .create_server("guest-server")
        .await
        .expect("server");
    SeaOrmServerMemberRepository::new(handle.clone())
        .add_member(server.id, owner.id, ServerMemberRole::Owner)
        .await
        .expect("owner");
    let channel = SeaOrmConversationRepository::new(handle.clone())
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Channel,
            title: Some("general".to_string()),
            is_private: false,
            created_by: Some(owner.id),
            encrypted: false,
        })
        .await
        .expect("channel");

    let invites = SeaOrmInviteRepository::new(handle.clone());
    for (token, role) in [
        ("rim_inv_guest_link", ServerMemberRole::Guest),
        ("rim_inv_member_link", ServerMemberRole::Member),
    ] {
        invites
            .create_invite(NewInvite {
                server_id: server.id,
                created_by_user_id: owner.id,
                token_hash: hash_token(token),
                expires_at: None,
                max_uses: 1,
                role,
            })
            .await
            .expect("invite");
    }

    let svc = build_auth_service(db.clone());
    for (user, token) in [
        (guest.id, "rim_inv_guest_link"),
        (member.id, "rim_inv_member_link"),
    ] {
        svc.join_server_by_invite(JoinServerByInviteCmd {
            actor: Actor::new(user),
            invite_token: token.to_string(),
        })
        .await
        .expect("join");
    }

    let members = SeaOrmServerMemberRepository::new(handle.clone());
    let conversation_members = SeaOrmConversationMemberRepository::new(handle.clone());
    let guest_member = members
        .get_member(server.id, guest.id)
        .await
        .expect("lookup")
        .expect("guest joined");
    assert_eq!(guest_member.member_role, ServerMemberRole::Guest);
    assert!(
        !conversation_members
            .is_member(channel.id, guest.id)
            .await
            .expect("lookup"),
        "guests are not auto-joined to public channels"
    );
    let plain_member = members
        .get_member(server.id, member.id)
        .await
        .expect("lookup")
        .expect("member joined");
    assert_eq!(plain_member.member_role, ServerMemberRole::Member);
    assert!(
        conversation_members
            .is_member(channel.id, member.id)
            .await
            .expect("lookup"),
        "members still auto-join public channels"
    );
}
