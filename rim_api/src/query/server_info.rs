use std::sync::Arc;

use async_graphql::*;
use rim_services::feature::service::FeatureGate;

use crate::types::ServerInfo;

#[derive(Default)]
pub(crate) struct ServerInfoQueries;

#[Object]
impl ServerInfoQueries {
    /// The server's release version, API version and features. Readable without a session.
    async fn server_info(&self, ctx: &Context<'_>) -> Result<ServerInfo> {
        let gate = ctx.data::<Arc<dyn FeatureGate>>()?;
        Ok(ServerInfo::current(gate.as_ref()))
    }
}
