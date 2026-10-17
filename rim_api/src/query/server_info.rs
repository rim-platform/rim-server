use std::sync::Arc;

use async_graphql::*;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::feature::service::FeatureGate;
use rim_services::limits::InstanceLimits;

use crate::types::{AuditRetention, InstancePolicy, ServerInfo};

#[derive(Default)]
pub(crate) struct ServerInfoQueries;

#[Object]
impl ServerInfoQueries {
    /// The server's release version, API version, features, audit retention, instance notice,
    /// terms URL, limits and DM settings. Readable without a session.
    async fn server_info(&self, ctx: &Context<'_>) -> Result<ServerInfo> {
        let gate = ctx.data::<Arc<dyn FeatureGate>>()?;
        let retention = ctx.data::<AuditRetention>()?;
        let policy = ctx.data::<InstancePolicy>()?;
        let limits = ctx.data::<InstanceLimits>()?;
        let dm_policy = ctx.data::<DmOperatorPolicy>()?;
        Ok(ServerInfo::current(
            gate.as_ref(),
            *retention,
            policy,
            limits,
            dm_policy,
        ))
    }
}
