use async_trait::async_trait;

use rim_core::server_member::model::ServerMemberRole;
use rim_services::server_member::command::{
    AddServerMemberCmd, GetServerMemberQuery, LeaveServerCmd, ListServerMembersQuery,
    RemoveServerMemberCmd,
};
use rim_services::server_member::dto::ServerMemberDto;
use rim_services::server_member::service::ServerMemberService;
use rim_services::{ServiceError, ServiceResult};

use crate::ServerMemberRepos;
use crate::util::{ensure_server_exists, ensure_server_member, ensure_server_owner};

#[cfg(test)]
use crate::test_support::server_member::{
    ServerMemberRepoPortAdapter, ServerMemberReposPorts, ServerRepoPortAdapter,
};

#[derive(Clone)]
pub struct DefaultServerMemberService {
    repos: ServerMemberRepos,
}

impl DefaultServerMemberService {
    pub fn new(repos: ServerMemberRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: ServerMemberReposPorts) -> Self {
        Self::new(ServerMemberRepos {
            server_repo: std::sync::Arc::new(ServerRepoPortAdapter(repos.server_repo)),
            server_member_repo: std::sync::Arc::new(ServerMemberRepoPortAdapter(
                repos.server_member_repo,
            )),
        })
    }

    fn to_dto(member: rim_core::server_member::model::ServerMember) -> ServerMemberDto {
        ServerMemberDto {
            server_id: member.server_id,
            user_id: member.user_id,
        }
    }
}

#[async_trait]
impl ServerMemberService for DefaultServerMemberService {
    async fn add_member(&self, cmd: AddServerMemberCmd) -> ServiceResult<ServerMemberDto> {
        let _server = ensure_server_exists(self.repos.server_repo.as_ref(), cmd.server_id).await?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        if cmd.user_id == cmd.actor.user_id {
            return Err(ServiceError::Validation(
                "cannot add yourself; you are already a member".to_string(),
            ));
        }

        let member = self
            .repos
            .server_member_repo
            .add_member(cmd.server_id, cmd.user_id, ServerMemberRole::Guest)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            server_id = %cmd.server_id,
            added_user_id = %cmd.user_id,
            "added server member"
        );

        Ok(Self::to_dto(member))
    }

    async fn remove_member(&self, cmd: RemoveServerMemberCmd) -> ServiceResult<()> {
        let _server = ensure_server_exists(self.repos.server_repo.as_ref(), cmd.server_id).await?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        if cmd.user_id == cmd.actor.user_id {
            return Err(ServiceError::Validation(
                "owner cannot remove themselves; use ownership transfer (not implemented)"
                    .to_string(),
            ));
        }

        let target = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if target.member_role == ServerMemberRole::Owner {
            return Err(ServiceError::Validation("cannot remove owner".to_string()));
        }

        self.repos
            .server_member_repo
            .remove_member(cmd.server_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            server_id = %cmd.server_id,
            removed_user_id = %cmd.user_id,
            "removed server member"
        );

        Ok(())
    }

    async fn leave(&self, cmd: LeaveServerCmd) -> ServiceResult<()> {
        let _server = ensure_server_exists(self.repos.server_repo.as_ref(), cmd.server_id).await?;

        let member = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::PermissionDenied)?;

        if member.member_role == ServerMemberRole::Owner {
            return Err(ServiceError::Validation(
                "owner cannot leave; transfer ownership first (not implemented)".to_string(),
            ));
        }

        self.repos
            .server_member_repo
            .remove_member(cmd.server_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            server_id = %cmd.server_id,
            user_id = %cmd.actor.user_id,
            "member left server"
        );

        Ok(())
    }

    async fn get_member(&self, query: GetServerMemberQuery) -> ServiceResult<ServerMemberDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let member = self
            .repos
            .server_member_repo
            .get_member(query.server_id, query.user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        Ok(Self::to_dto(member))
    }

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let members = self
            .repos
            .server_member_repo
            .get_members_for_server(query.server_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(members.into_iter().map(Self::to_dto).collect())
    }
}
use tracing::info;
