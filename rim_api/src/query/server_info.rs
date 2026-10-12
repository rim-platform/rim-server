use std::sync::Arc;

use async_graphql::*;
use rim_services::feature::service::FeatureGate;

use crate::types::{AuditRetention, ServerInfo};

#[derive(Default)]
pub(crate) struct ServerInfoQueries;

#[Object]
impl ServerInfoQueries {
    /// The server's release version, API version, features and audit retention. Readable
    /// without a session.
    async fn server_info(&self, ctx: &Context<'_>) -> Result<ServerInfo> {
        let gate = ctx.data::<Arc<dyn FeatureGate>>()?;
        let retention = ctx.data::<AuditRetention>()?;
        Ok(ServerInfo::current(gate.as_ref(), *retention))
    }
}
