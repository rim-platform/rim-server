use crate::config::RIMConfig;
use rim_api::{AppSchema, build_schema};
use sea_orm::Database;
use std::sync::Arc;

use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl;
use rim_infra_db::sea_orm_impl::{
    invite::SeaOrmInviteRepository, keybind::SeaOrmKeybindRepository,
    server::SeaOrmServerRepository, server_member::SeaOrmServerMemberRepository,
    session::SeaOrmSessionRepository, user::SeaOrmUserRepository,
    user_command::SeaOrmUserCommandRepository,
};
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_services::auth::service::RequestAuthenticator;
use rim_services::provider::AppServiceProvider;

pub struct AppRuntime {
    pub schema: AppSchema,
    pub request_authenticator: Arc<dyn RequestAuthenticator>,
}

pub async fn build_app_runtime(config: &RIMConfig) -> std::io::Result<AppRuntime> {
    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let raw_db = db.clone(); // Keep raw DatabaseConnection for UoW
    let db: DbHandle = db.into();

    let user_repo: Arc<dyn rim_core::user::repository::UserRepository> =
        Arc::new(SeaOrmUserRepository::new(db.clone()));
    let server_repo: Arc<dyn rim_core::server::repository::ServerRepository> =
        Arc::new(SeaOrmServerRepository::new(db.clone()));
    let server_member_repo: Arc<dyn rim_core::server_member::repository::ServerMemberRepository> =
        Arc::new(SeaOrmServerMemberRepository::new(db.clone()));
    let session_repo: Arc<dyn rim_core::auth::repository::SessionRepository> =
        Arc::new(SeaOrmSessionRepository::new(db.clone()));
    let invite_repo: Arc<dyn rim_core::auth::repository::InviteRepository> =
        Arc::new(SeaOrmInviteRepository::new(db.clone()));

    let repos = AppRepos {
        conversation_repo: Arc::new(
            sea_orm_impl::conversation::SeaOrmConversationRepository::new(db.clone()),
        ),
        conversation_member_repo: Arc::new(
            sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository::new(db.clone()),
        ),
        keybind_repo: Arc::new(SeaOrmKeybindRepository::new(db.clone())),
        message_repo: Arc::new(sea_orm_impl::message::SeaOrmMessageRepository::new(
            db.clone(),
        )),
        message_attachment_repo: Arc::new(
            sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository::new(db.clone()),
        ),
        message_thread_repo: Arc::new(
            sea_orm_impl::message_thread::SeaOrmMessageThreadRepository::new(db.clone()),
        ),
        user_command_repo: Arc::new(SeaOrmUserCommandRepository::new(db.clone())),
        server_repo: Arc::clone(&server_repo),
        server_member_repo: Arc::clone(&server_member_repo),
        user_repo: Arc::clone(&user_repo),
        session_repo: Arc::clone(&session_repo),
        invite_repo: Arc::clone(&invite_repo),
    };

    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        config.bootstrap_secret.clone(),
        chrono::Duration::days(i64::from(config.session_ttl_days)),
    ));
    let schema = build_schema(Arc::clone(&service_provider));
    let request_authenticator = service_provider.request_authenticator();

    Ok(AppRuntime {
        schema,
        request_authenticator,
    })
}
