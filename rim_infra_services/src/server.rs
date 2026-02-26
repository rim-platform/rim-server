use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::server_member::model::ServerMemberRole;
use rim_services::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
use rim_services::server::dto::ServerDto;
use rim_services::server::service::ServerService;
use rim_services::{ServiceError, ServiceResult};

use crate::ServerRepos;
use crate::util::{ensure_server_exists, ensure_server_member};

#[cfg(test)]
use crate::test_support::server::{
    ServerMemberRepoPortAdapter, ServerRepoPortAdapter, ServerReposPorts,
};

#[derive(Clone)]
pub struct DefaultServerService {
    repos: ServerRepos,
}

impl DefaultServerService {
    pub fn new(repos: ServerRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: ServerReposPorts) -> Self {
        Self::new(ServerRepos {
            server_repo: std::sync::Arc::new(ServerRepoPortAdapter(repos.server_repo)),
            server_member_repo: std::sync::Arc::new(ServerMemberRepoPortAdapter(
                repos.server_member_repo,
            )),
        })
    }

    fn to_dto(server: rim_core::server::model::Server) -> ServerDto {
        ServerDto {
            id: server.id,
            name: server.name,
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
        if name.len() > 128 {
            return Err(ServiceError::Validation(
                "server name is too long".to_string(),
            ));
        }

        let server = self
            .repos
            .server_repo
            .create_server(name)
            .await
            .map_err(rim_services::map_repo_error)?;

        // Creator becomes owner.
        let _ = self
            .repos
            .server_member_repo
            .add_member(server.id, cmd.actor.user_id, ServerMemberRole::Owner)
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(servers.into_iter().map(Self::to_dto).collect())
    }
}
