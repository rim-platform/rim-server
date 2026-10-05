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
use crate::util::{
    add_workspace_member, ensure_not_banned, ensure_server_exists, ensure_server_owner,
    member_standing,
};

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
        timeout_until: Option<chrono::DateTime<chrono::Utc>>,
    ) -> ServerMemberDto {
        ServerMemberDto {
            server_id: member.server_id,
            user_id: member.user_id,
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar_url.clone(),
            status_text: user.status_text.clone(),
            bio: user.bio.clone(),
            member_role: member.member_role,
            joined_at: member.joined_at,
            timeout_until,
        }
    }

    /// The users a guest may see in the member directory (Track 1.2): themselves and everyone
    /// who shares a conversation with them. `None` for non-guests (everyone is visible).
    async fn visible_member_ids(
        &self,
        server_id: rim_core::server::model::ServerId,
        actor: rim_core::user::model::UserId,
    ) -> ServiceResult<Option<std::collections::HashSet<rim_core::user::model::UserId>>> {
        let standing =
            member_standing(self.repos.server_member_repo.as_ref(), server_id, actor).await?;
        if !standing.role.is_guest() {
            return Ok(None);
        }
        let joined = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(server_id, actor)
            .await?;
        let mut visible: std::collections::HashSet<_> = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&joined)
            .await?
            .into_values()
            .flatten()
            .collect();
        visible.insert(actor);
        Ok(Some(visible))
    }

    async fn active_timeout_until(
        &self,
        server_id: rim_core::server::model::ServerId,
        user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self
            .repos
            .moderation_repo
            .get_active_timeout(server_id, user_id, chrono::Utc::now())
            .await?
            .map(|t| t.until))
    }

    async fn resolve_user(
        &self,
        user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<rim_core::user::model::User> {
        self.repos
            .user_repo
            .get_by_id(user_id)
            .await?
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
        ensure_not_banned(
            self.repos.moderation_repo.as_ref(),
            cmd.server_id,
            cmd.user_id,
        )
        .await?;

        let member = add_workspace_member(
            self.repos.server_member_repo.as_ref(),
            self.repos.conversation_repo.as_ref(),
            self.repos.conversation_member_repo.as_ref(),
            cmd.server_id,
            cmd.user_id,
            ServerMemberRole::Member,
        )
        .await?;

        let user = self.resolve_user(member.user_id).await?;

        info!(
            server_id = %cmd.server_id,
            added_user_id = %cmd.user_id,
            "added server member"
        );

        Ok(Self::to_dto(member, &user, None))
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
            .await?
            .ok_or(ServiceError::NotFound)?;

        if target.member_role == ServerMemberRole::Owner {
            return Err(ServiceError::Validation("cannot remove owner".to_string()));
        }

        self.repos
            .server_member_repo
            .remove_member(cmd.server_id, cmd.user_id)
            .await?;

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
            .await?
            .ok_or(ServiceError::PermissionDenied)?;

        if member.member_role == ServerMemberRole::Owner {
            return Err(ServiceError::Validation(
                "owner cannot leave; transfer ownership first (not implemented)".to_string(),
            ));
        }

        self.repos
            .server_member_repo
            .remove_member(cmd.server_id, cmd.actor.user_id)
            .await?;

        info!(
            server_id = %cmd.server_id,
            user_id = %cmd.actor.user_id,
            "member left server"
        );

        Ok(())
    }

    async fn get_member(&self, query: GetServerMemberQuery) -> ServiceResult<ServerMemberDto> {
        let visible = self
            .visible_member_ids(query.server_id, query.actor.user_id)
            .await?;
        if visible
            .as_ref()
            .is_some_and(|ids| !ids.contains(&query.user_id))
        {
            return Err(ServiceError::NotFound);
        }

        let member = self
            .repos
            .server_member_repo
            .get_member(query.server_id, query.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        let user = self.resolve_user(member.user_id).await?;
        let timeout_until = self
            .active_timeout_until(query.server_id, query.user_id)
            .await?;

        Ok(Self::to_dto(member, &user, timeout_until))
    }

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>> {
        // Guests see only members who share a conversation with them (Track 1.2).
        let visible = self
            .visible_member_ids(query.server_id, query.actor.user_id)
            .await?;

        let mut members = self
            .repos
            .server_member_repo
            .get_members_for_server(query.server_id)
            .await?;
        if let Some(visible) = &visible {
            members.retain(|m| visible.contains(&m.user_id));
        }
        let timeouts: std::collections::HashMap<_, _> = self
            .repos
            .moderation_repo
            .list_active_timeouts(query.server_id, chrono::Utc::now())
            .await?
            .into_iter()
            .map(|t| (t.user_id, t.until))
            .collect();

        // Batch-fetch all users in a single query to avoid N+1
        let user_ids: Vec<rim_core::user::model::UserId> =
            members.iter().map(|m| m.user_id).collect();
        let users = self.repos.user_repo.get_by_ids(&user_ids).await?;

        let user_map: std::collections::HashMap<_, _> =
            users.into_iter().map(|u| (u.id, u)).collect();

        let mut result = Vec::with_capacity(members.len());
        for member in members {
            let user = user_map
                .get(&member.user_id)
                .ok_or(ServiceError::NotFound)?;
            let timeout_until = timeouts.get(&member.user_id).copied();
            result.push(Self::to_dto(member, user, timeout_until));
        }

        Ok(result)
    }
}
use tracing::info;
