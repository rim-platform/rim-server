use async_trait::async_trait;

use rim_services::server_member::command::{GetServerMemberQuery, ListServerMembersQuery};
use rim_services::server_member::dto::ServerMemberDto;
use rim_services::server_member::service::ServerMemberService;
use rim_services::{ServiceError, ServiceResult};

use crate::ServerMemberRepos;
use crate::util::member_standing;

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
            principal_id: user.principal_id.clone(),
        }
    }

    /// The users a guest may see in the member directory: themselves and everyone
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

    async fn visible_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Option<std::collections::HashSet<rim_core::user::model::UserId>>> {
        self.visible_member_ids(query.server_id, query.actor.user_id)
            .await
    }

    async fn list_members(
        &self,
        query: ListServerMembersQuery,
    ) -> ServiceResult<Vec<ServerMemberDto>> {
        // Guests see only members who share a conversation with them.
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
