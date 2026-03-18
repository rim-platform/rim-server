#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::{NewInvite, NewSession};
use rim_core::auth::repository::{InviteRepository, SessionRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;

#[tokio::test]
async fn map_db_err_unique_violation_is_conflict_with_message() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let member_repo = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let invite_repo = SeaOrmInviteRepository::new(ctx.db.clone());

    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;
    let _ = member_repo
        .add_member(server.id, user.id, ServerMemberRole::Owner)
        .await
        .unwrap();

    let _ = invite_repo
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: user.id,
            token_hash: "dup".to_string(),
            expires_at: None,
            max_uses: 1,
        })
        .await
        .unwrap();

    let err = invite_repo
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: user.id,
            token_hash: "dup".to_string(),
            expires_at: None,
            max_uses: 1,
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), RepoErrorKind::Conflict);
}

#[tokio::test]
async fn map_db_err_row_not_found_is_not_found() {
    let Some(_ctx) = support::TestCtx::new().await else {
        return;
    };

    let err = sea_orm::DbErr::RecordNotFound("missing".to_string());
    let mapped = rim_infra_db::map_db_err(err);
    assert_eq!(mapped.kind(), RepoErrorKind::NotFound);
}

#[tokio::test]
async fn map_db_err_fk_violation_is_conflict() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let session_repo = SeaOrmSessionRepository::new(ctx.db.clone());

    let err = session_repo
        .create_session(NewSession {
            user_id: rim_core::user::model::UserId::new(),
            token_hash: "fk".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap_err();

    assert_eq!(err.kind(), RepoErrorKind::Conflict);
}
