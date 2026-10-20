use std::sync::Arc;

use async_graphql::*;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::feature::service::FeatureGate;
use rim_services::limits::InstanceLimits;

use crate::errors::map_service_error;
use crate::types::{AuditRetention, InstancePolicy, ServerInfo};

#[derive(Default)]
pub(crate) struct ServerInfoQueries;

#[Object]
impl ServerInfoQueries {
    /// The server's release version, API version, features, audit retention, instance notice,
    /// terms URL, limits, DM settings, abuse contact, minimum age and default workspace. Readable
    /// without a session.
    async fn server_info(&self, ctx: &Context<'_>) -> Result<ServerInfo> {
        let gate = ctx.data::<Arc<dyn FeatureGate>>()?;
        let retention = ctx.data::<AuditRetention>()?;
        let policy = ctx.data::<InstancePolicy>()?;
        let limits = ctx.data::<InstanceLimits>()?;
        let dm_policy = ctx.data::<DmOperatorPolicy>()?;
        let mut info = ServerInfo::current(gate.as_ref(), *retention, policy, limits, dm_policy);
        // Without a configured workspace, the one an owner claimed from the definition file.
        if info.default_workspace.is_none()
            && let Ok(default_workspace) = ctx.data::<Arc<dyn DefaultWorkspaceService>>()
        {
            info.default_workspace = default_workspace
                .current()
                .await
                .map_err(map_service_error)?
                .map(|workspace| ID(workspace.to_string()));
        }
        Ok(info)
    }
}
