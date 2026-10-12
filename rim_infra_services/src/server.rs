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
use rim_services::limits::{InstanceLimits, ensure_below};
use rim_services::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
use rim_services::server::dto::ServerDto;
use rim_services::server::service::ServerService;
use rim_services::{ServiceError, ServiceResult};

use crate::ServerRepos;
use crate::clock::Clock;
use crate::repos::AppRepos;
use crate::util::{add_workspace_member, ensure_server_exists, ensure_server_member};

#[derive(Clone)]
pub struct DefaultServerService {
    repos: ServerRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    /// Caps on workspaces per owner and on the default channels, and the probation of new
    /// accounts.
    limits: InstanceLimits,
    clock: Clock,
}

impl DefaultServerService {
    pub fn new(repos: ServerRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
            limits: InstanceLimits::default(),
            clock: Clock::default(),
        }
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// `Validation` while the creator's account is in probation.
    async fn ensure_out_of_probation(&self, user_id: UserId) -> ServiceResult<()> {
        if self.limits.probation_hours.is_none() {
            return Ok(());
        }
        let user = self
            .repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.limits
            .ensure_out_of_probation(user.created_at, self.clock.now(), "create a workspace")
    }

    fn to_dto(server: rim_core::server::model::Server) -> ServerDto {
        ServerDto {
            id: server.id,
            name: server.name,
        }
    }
}

/// Create default #general and #random channels for a newly created server, as many of them
/// as `max_channels` allows.
///
/// If channel creation fails, a warning is logged but the error is **not**
/// propagated — we never want a default-channel failure to abort server
/// creation.
async fn create_default_channels(
    conversation_repo: &dyn ConversationRepository,
    observers: &Observers,
    server_id: ServerId,
    creator_id: UserId,
    max_channels: Option<u32>,
) {
    let allowed = max_channels.map_or(usize::MAX, |n| n as usize);
    for channel_name in ["general", "random"].iter().take(allowed) {
        match conversation_repo
            .create_conversation_with_members(
                NewConversation {
                    server_id,
                    conversation_type: ConversationType::Channel,
                    title: Some(channel_name.to_string()),
                    is_private: false,
                    created_by: Some(creator_id),
                    encrypted: false,
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
        if let Some(existing) = self
            .repos
            .server_repo
            .find_owned_by_name(cmd.actor.user_id, name)
            .await?
        {
            return Err(ServiceError::Conflict(format!(
                "you already own a workspace named \"{}\"",
                existing.name
            )));
        }
        self.ensure_out_of_probation(cmd.actor.user_id).await?;
        if self.limits.workspaces_per_user.is_some() {
            let owned = self
                .repos
                .server_repo
                .count_owned(cmd.actor.user_id)
                .await?;
            ensure_below(
                self.limits.workspaces_per_user,
                owned,
                "workspaces per user",
            )?;
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
        let _ = add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            server.id,
            cmd.actor.user_id,
            ServerMemberRole::Owner,
            None,
        )
        .await?;

        // Create default channels inside the transaction (if available).
        create_default_channels(
            repos.conversation_repo.as_ref(),
            &self.observers,
            server.id,
            cmd.actor.user_id,
            self.limits.channels_per_workspace,
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
