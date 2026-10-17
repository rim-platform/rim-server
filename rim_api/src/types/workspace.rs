use async_graphql::{ComplexObject, Context, Enum, ID, InputObject, Result, SimpleObject};

use crate::resolve::ResolveExt;
use crate::types::User;

/// A member's role in a workspace, from most to least privileged. Used by `Server.viewerRole`,
/// `ServerMember.memberRole`, `CreateInviteInput.role` and `setMemberRole`.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ServerMemberRoleGql {
    Owner,
    Moderator,
    Member,
    Guest,
}

impl From<rim_core::server_member::model::ServerMemberRole> for ServerMemberRoleGql {
    fn from(value: rim_core::server_member::model::ServerMemberRole) -> Self {
        use rim_core::server_member::model::ServerMemberRole as D;
        match value {
            D::Owner => Self::Owner,
            D::Moderator => Self::Moderator,
            D::Member => Self::Member,
            D::Guest => Self::Guest,
        }
    }
}

impl From<ServerMemberRoleGql> for rim_core::server_member::model::ServerMemberRole {
    fn from(value: ServerMemberRoleGql) -> Self {
        match value {
            ServerMemberRoleGql::Owner => Self::Owner,
            ServerMemberRoleGql::Moderator => Self::Moderator,
            ServerMemberRoleGql::Member => Self::Member,
            ServerMemberRoleGql::Guest => Self::Guest,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Server {
    pub id: async_graphql::ID,
    pub name: String,
}

#[ComplexObject]
impl Server {
    /// The requesting actor's effective role in this server. **Display/affordance signal
    /// only — not an authorization boundary.** Every mutation independently re-checks the
    /// role server-side (defense in depth); this field just lets the client avoid rendering
    /// controls the viewer can't use. Non-null: every viewer who can query a `Server` is
    /// necessarily a member of it (query-level visibility already guards non-members), so
    /// there is no "not a member" variant to represent here.
    async fn viewer_role(&self, ctx: &Context<'_>) -> Result<ServerMemberRoleGql> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>(
            )?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&self.id)?;

        let member = svc
            .get_member(rim_services::server_member::command::GetServerMemberQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
                user_id: actor,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(member.member_role.into())
    }

    /// The account holding the owner role; null when there is none, as in the direct space or
    /// once the owner's account is deleted. Workspace metadata: every member sees it, guests
    /// included.
    async fn owner(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>(
            )?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&self.id)?;

        let owner = svc
            .get_owner(rim_services::server_member::command::GetServerOwnerQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        match owner {
            Some(id) => ctx.load_user(&ID(id.to_string())).await,
            None => Ok(None),
        }
    }

    /// The workspace's floor for DMs to its members from people outside their workspaces;
    /// null in the direct space, which has no rules. Every member sees it.
    async fn dm_outside_floor(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<crate::types::DmOutsideFloor>> {
        Ok(dm_policy_of(ctx, &self.id)
            .await?
            .map(|p| p.outside_floor.into()))
    }

    /// How a DM from a fellow member arrives unless the recipient chose; null in the direct
    /// space. Every member sees it.
    async fn dm_member_default(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<crate::types::DmMembers>> {
        Ok(dm_policy_of(ctx, &self.id)
            .await?
            .map(|p| p.member_default.into()))
    }
}

/// A workspace's DM rules for the caller, a member; `None` in the direct space, which has none.
async fn dm_policy_of(
    ctx: &Context<'_>,
    id: &ID,
) -> Result<Option<rim_services::dm_policy::dto::WorkspaceDmPolicyDto>> {
    let actor = crate::auth_context::require_actor(ctx)?;
    let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(id)?;
    match ctx
        .data::<std::sync::Arc<dyn rim_services::dm_policy::service::DmPolicyService>>()?
        .workspace_dm_policy(rim_services::Actor::new(actor), server_id)
        .await
    {
        Ok(policy) => Ok(Some(policy)),
        Err(rim_services::ServiceError::NotFound) => Ok(None),
        Err(e) => Err(crate::errors::map_service_error(e)),
    }
}

impl From<rim_services::server::dto::ServerDto> for Server {
    fn from(value: rim_services::server::dto::ServerDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            name: value.name,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ServerMember {
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    pub member_role: ServerMemberRoleGql,
    pub joined_at: chrono::DateTime<chrono::Utc>,
    /// End of the member's timeout while one is in force, else null.
    pub timeout_until: Option<chrono::DateTime<chrono::Utc>>,
    /// The member's key identity (`rimp1_…`), once enrolled; null for a password-only account.
    pub principal_id: Option<String>,
}

impl From<rim_services::server_member::dto::ServerMemberDto> for ServerMember {
    fn from(value: rim_services::server_member::dto::ServerMemberDto) -> Self {
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            username: value.username,
            display_name: value.display_name,
            avatar_url: value.avatar_url,
            status_text: value.status_text,
            bio: value.bio,
            member_role: value.member_role.into(),
            joined_at: value.joined_at,
            timeout_until: value.timeout_until,
            principal_id: value.principal_id,
        }
    }
}

/// Hand a workspace's ownership to another member.
#[derive(InputObject)]
pub struct TransferWorkspaceOwnershipInput {
    pub server_id: ID,
    /// The member, not a guest, who becomes the owner.
    pub user_id: ID,
}

/// Delete a workspace and everything in it.
#[derive(InputObject)]
pub struct DeleteWorkspaceInput {
    pub server_id: ID,
    /// The workspace's name, typed again to confirm.
    pub confirm_name: String,
}

#[derive(SimpleObject, Clone, Copy, Debug)]
pub struct DeleteWorkspacePayload {
    /// Whether the workspace was deleted.
    pub deleted: bool,
}
