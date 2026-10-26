use std::sync::Arc;

use async_graphql::*;
use rim_core::relay::model::AbuseReportId;
use rim_services::ServiceError;
use rim_services::ip_block::dto::BlockIpCmd;
use rim_services::ip_block::service::IpBlockService;
use rim_services::relay::service::RelayAbuseService;

use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::query::instance_moderation::operator_caller;
use crate::types::{
    InstanceAbuseReport, InstanceIpBlockOutcome, InstancePrincipalBlockOutcome,
    InstancePrincipalUnblockOutcome, InstanceProfileClearOutcome,
};

#[derive(Default)]
pub(crate) struct InstanceModerationMutations;

#[Object]
impl InstanceModerationMutations {
    /// Block a principal on this server: its pending DM requests are ignored, and the account
    /// holding it here has its sessions ended, its invites with uses left revoked and its
    /// email refused. Returns one outcome per account blocked, the principal's first. Instance
    /// operators only; anyone else gets `FORBIDDEN`.
    async fn instance_block_principal(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The principal to block (`rimp1_…`).")] principal_id: String,
        #[graphql(desc = "Why it is blocked, 1 to 1000 characters.")] reason: String,
        #[graphql(
            default = false,
            desc = "Also block every account its invites created, recursively."
        )]
        cascade: bool,
    ) -> Result<Vec<InstancePrincipalBlockOutcome>> {
        let by = operator_caller(ctx)?;
        let outcomes = ctx
            .data::<Arc<dyn RelayAbuseService>>()?
            .block_principal(by, principal_id, reason, cascade)
            .await
            .map_err(map_service_error)?;
        Ok(outcomes.into_iter().map(Into::into).collect())
    }

    /// Lift a principal's block and its account's email block. Ended sessions and revoked
    /// invites stay ended. Instance operators only; anyone else gets `FORBIDDEN`.
    async fn instance_unblock_principal(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The principal to unblock (`rimp1_…`).")] principal_id: String,
    ) -> Result<InstancePrincipalUnblockOutcome> {
        let by = operator_caller(ctx)?;
        Ok(ctx
            .data::<Arc<dyn RelayAbuseService>>()?
            .unblock_principal(by, principal_id)
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Mark an abuse report resolved; `NOT_FOUND` when there is no such report. Instance
    /// operators only; anyone else gets `FORBIDDEN`.
    async fn instance_resolve_report(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The report's id.")] id: ID,
    ) -> Result<InstanceAbuseReport> {
        let by = operator_caller(ctx)?;
        let id = parse_uuid_id::<AbuseReportId>(&id)?;
        ctx.data::<Arc<dyn RelayAbuseService>>()?
            .resolve_report(by, id)
            .await
            .map_err(map_service_error)?
            .map(Into::into)
            .ok_or_else(|| map_service_error(ServiceError::NotFound))
    }

    /// Block an address or range, or replace the reason and expiry of its block. A running
    /// server applies it within 30 seconds. Instance operators only; anyone else gets
    /// `FORBIDDEN`.
    async fn instance_block_ip(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "An address or a CIDR range, IPv4 or IPv6.")] cidr: String,
        #[graphql(desc = "Why it is blocked, 1 to 1000 characters.")] reason: String,
        #[graphql(desc = "Days until the block lifts, 1 to 365; null blocks until it is removed.")]
        expires_in_days: Option<u32>,
    ) -> Result<InstanceIpBlockOutcome> {
        let by = operator_caller(ctx)?;
        let outcome = ctx
            .data::<Arc<dyn IpBlockService>>()?
            .block(
                by,
                BlockIpCmd {
                    cidr,
                    reason,
                    expires_in_days,
                },
            )
            .await
            .map_err(map_service_error)?;
        Ok(InstanceIpBlockOutcome::at(outcome, chrono::Utc::now()))
    }

    /// Lift the block on an address or range, named as it was blocked or in any form naming
    /// the same range. Returns whether there was one. A running server applies it within 30
    /// seconds. Instance operators only; anyone else gets `FORBIDDEN`.
    async fn instance_unblock_ip(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The address or CIDR range to unblock.")] cidr: String,
    ) -> Result<bool> {
        let by = operator_caller(ctx)?;
        ctx.data::<Arc<dyn IpBlockService>>()?
            .unblock(by, cidr)
            .await
            .map_err(map_service_error)
    }

    /// Clear the display name, picture, status and bio of the account holding a principal on
    /// this server, deleting its stored pictures, and optionally lock them: until the lock
    /// ends the account can only clear them. Another server keeps showing the display name in
    /// its copy of the person's signed profile card. `NOT_FOUND` when no account here holds the
    /// principal. Instance operators only; anyone else gets `FORBIDDEN`.
    async fn instance_clear_profile(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "The principal whose account to clear (`rimp1_…`).")] principal_id: String,
        #[graphql(desc = "Why it is cleared, 1 to 1000 characters; written to the audit log.")]
        reason: String,
        #[graphql(desc = "Days to lock the profile for, 1 to 365; null leaves it unlocked.")]
        lock_days: Option<u32>,
    ) -> Result<InstanceProfileClearOutcome> {
        let by = operator_caller(ctx)?;
        Ok(ctx
            .data::<Arc<dyn RelayAbuseService>>()?
            .clear_profile(by, principal_id, reason, lock_days)
            .await
            .map_err(map_service_error)?
            .into())
    }
}
