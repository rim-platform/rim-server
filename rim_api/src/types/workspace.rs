use async_graphql::{ComplexObject, Context, Enum, Result, SimpleObject};

use super::WorkspaceFeatureState;

/// GraphQL exposure of `rim_core::server_member::model::ServerMemberRole`. Reused (not
/// duplicated) across every place a member's role needs to cross the wire — `Server.viewerRole`,
/// `ServerMember.memberRole`, `CreateInviteInput.role`, `setMemberRole`.
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

    /// Per-workspace optional modules and whether each is on (Track 0.4). One entry per known
    /// feature, defaults filled in. Resolved with one service call per server, like
    /// `viewerRole`; `myServers` lists few servers per user, so no DataLoader.
    async fn features(&self, ctx: &Context<'_>) -> Result<Vec<WorkspaceFeatureState>> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<
                dyn rim_services::workspace_feature::service::WorkspaceFeatureService,
            >>()?;
        let server_id = crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>(&self.id)?;

        let states = svc
            .list(
                rim_services::workspace_feature::command::ListWorkspaceFeaturesQuery {
                    actor: rim_services::Actor::new(actor),
                    server_id,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(states.into_iter().map(Into::into).collect())
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
    /// End of the member's timeout while one is in force (Track 1.2), else null.
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
