use std::sync::Arc;

use async_graphql::*;
use rim_services::Actor;
use rim_services::ip_block::service::IpBlockService;
use rim_services::operator::OperatorCaller;
use rim_services::relay::service::RelayAbuseService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::types::{InstanceAbuseReport, InstanceIpBlock, InstancePrincipalBlock};

/// The signed-in viewer as the caller of an instance operator's tool.
pub(crate) fn operator_caller(ctx: &Context<'_>) -> Result<OperatorCaller> {
    Ok(OperatorCaller::User(Actor::new(require_actor(ctx)?)))
}

#[derive(Default)]
pub(crate) struct InstanceModerationQueries;

#[Object]
impl InstanceModerationQueries {
    /// Abuse reports filed with this server, oldest first. Instance operators only; anyone
    /// else gets `FORBIDDEN`.
    async fn instance_reports(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = true, desc = "Only the reports not yet resolved.")] open_only: bool,
    ) -> Result<Vec<InstanceAbuseReport>> {
        let by = operator_caller(ctx)?;
        let reports = ctx
            .data::<Arc<dyn RelayAbuseService>>()?
            .reports(by, open_only)
            .await
            .map_err(map_service_error)?;
        Ok(reports.into_iter().map(Into::into).collect())
    }

    /// The principals this server refuses, newest first. Instance operators only; anyone
    /// else gets `FORBIDDEN`.
    async fn instance_principal_blocks(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<InstancePrincipalBlock>> {
        let by = operator_caller(ctx)?;
        let blocks = ctx
            .data::<Arc<dyn RelayAbuseService>>()?
            .principal_blocks(by)
            .await
            .map_err(map_service_error)?;
        Ok(blocks.into_iter().map(Into::into).collect())
    }

    /// This server's IP deny list, expired blocks included, oldest first. Instance operators
    /// only; anyone else gets `FORBIDDEN`.
    async fn instance_ip_blocks(&self, ctx: &Context<'_>) -> Result<Vec<InstanceIpBlock>> {
        let by = operator_caller(ctx)?;
        let blocks = ctx
            .data::<Arc<dyn IpBlockService>>()?
            .list(by)
            .await
            .map_err(map_service_error)?;
        let now = chrono::Utc::now();
        Ok(blocks
            .into_iter()
            .map(|b| InstanceIpBlock::at(b, now))
            .collect())
    }
}
