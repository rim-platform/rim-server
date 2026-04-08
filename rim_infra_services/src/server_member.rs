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

#[derive(Clone)]
pub struct DefaultServerMemberService {
    repos: ServerMemberRepos,
}

impl DefaultServerMemberService {
    pub fn new(repos: ServerMemberRepos) -> Self {
        Self { repos }
    }

    fn to_dto(
        member: rim_core::server_member::model::ServerMember,
        user: &rim_core::user::model::User,
    ) -> ServerMemberDto {
        ServerMemberDto {
            server_id: member.server_id,
            user_id: member.user_id,
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar_url.clone(),
            status_text: user.status_text.clone(),
            bio: user.bio.clone(),
            member_role: member.member_role.as_str().to_string(),
            joined_at: member.joined_at,
        }
    }

    async fn resolve_user(
        &self,
        user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<rim_core::user::model::User> {
        self.repos
            .user_repo
            .get_by_id(user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)
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

        let user = self.resolve_user(member.user_id).await?;

        info!(
            server_id = %cmd.server_id,
            added_user_id = %cmd.user_id,
            "added server member"
        );

        Ok(Self::to_dto(member, &user))
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

        let user = self.resolve_user(member.user_id).await?;

        Ok(Self::to_dto(member, &user))
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

        // Batch-fetch all users in a single query to avoid N+1
        let user_ids: Vec<rim_core::user::model::UserId> =
            members.iter().map(|m| m.user_id).collect();
        let users = self
            .repos
            .user_repo
            .get_by_ids(&user_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let user_map: std::collections::HashMap<_, _> =
            users.into_iter().map(|u| (u.id, u)).collect();

        let mut result = Vec::with_capacity(members.len());
        for member in members {
            let user = user_map
                .get(&member.user_id)
                .ok_or(ServiceError::NotFound)?;
            result.push(Self::to_dto(member, user));
        }

        Ok(result)
    }
}
use tracing::info;
