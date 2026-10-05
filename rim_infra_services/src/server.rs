use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
use rim_services::server::dto::ServerDto;
use rim_services::server::service::ServerService;
use rim_services::{ServiceError, ServiceResult};

use crate::ServerRepos;
use crate::repos::AppRepos;
use crate::util::{ensure_server_exists, ensure_server_member};

#[derive(Clone)]
pub struct DefaultServerService {
    repos: ServerRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
}

impl DefaultServerService {
    pub fn new(repos: ServerRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
        }
    }

    fn to_dto(server: rim_core::server::model::Server) -> ServerDto {
        ServerDto {
            id: server.id,
            name: server.name,
        }
    }
}

/// Create default #general and #random channels for a newly created server.
///
/// If channel creation fails, a warning is logged but the error is **not**
/// propagated — we never want a default-channel failure to abort server
/// creation.
async fn create_default_channels(
    conversation_repo: &dyn ConversationRepository,
    observers: &Observers,
    server_id: ServerId,
    creator_id: UserId,
) {
    for channel_name in &["general", "random"] {
        match conversation_repo
            .create_conversation_with_members(
                NewConversation {
                    server_id,
                    conversation_type: ConversationType::Channel,
                    title: Some(channel_name.to_string()),
                    is_private: false,
                    created_by: Some(creator_id),
                },
                &[creator_id],
            )
            .await
        {
            Ok(conv) => {
                info!(
                    server_id = %server_id,
                    conversation_id = %conv.id,
                    channel = %channel_name,
                    "created default channel"
                );
                observers.emit(WorkspaceEvent::ConversationCreated {
                    workspace_id: server_id,
                    conversation_id: conv.id,
                    title: conv.title.clone(),
                    conversation_type: conv.conversation_type.as_str().to_string(),
                    member_user_ids: vec![creator_id],
                    is_private: conv.is_private,
                });
            }
            Err(err) => {
                warn!(
                    server_id = %server_id,
                    channel = %channel_name,
                    error = %err,
                    "failed to create default channel"
                );
            }
        }
    }
}

#[async_trait]
impl ServerService for DefaultServerService {
    #[instrument(name = "server.create_server", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn create_server(&self, cmd: CreateServerCmd) -> ServiceResult<ServerDto> {
        let name = cmd.name.trim();
        if name.is_empty() {
            return Err(ServiceError::Validation(
                "server name cannot be empty".to_string(),
            ));
        }
        if name.chars().count() > 128 {
            return Err(ServiceError::Validation(
                "server name is too long".to_string(),
            ));
        }

        // Run create + add_member inside a transaction when DB is available.
        let scope = if let Some(db) = &self.db {
            Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            )
        } else {
            None
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_server(),
            None => self.repos.clone(),
        };

        let server = repos.server_repo.create_server(name).await?;

        // Creator becomes owner.
        let _ = repos
            .server_member_repo
            .add_member(server.id, cmd.actor.user_id, ServerMemberRole::Owner)
            .await?;

        // Create default channels inside the transaction (if available).
        create_default_channels(
            repos.conversation_repo.as_ref(),
            &self.observers,
            server.id,
            cmd.actor.user_id,
        )
        .await;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        info!(server_id = %server.id, "created server");

        Ok(Self::to_dto(server))
    }

    async fn get_server(&self, query: GetServerQuery) -> ServiceResult<ServerDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let server = ensure_server_exists(self.repos.server_repo.as_ref(), query.server_id).await?;
        Ok(Self::to_dto(server))
    }

    async fn list_my_servers(&self, query: ListMyServersQuery) -> ServiceResult<Vec<ServerDto>> {
        let servers = self
            .repos
            .server_repo
            .get_servers_for_user(query.actor.user_id)
            .await?;

        Ok(servers.into_iter().map(Self::to_dto).collect())
    }
}
