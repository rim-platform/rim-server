use async_graphql::*;
use rim_core::e2ee::model::MlsWelcomeCursor;
use rim_services::Actor;
use rim_services::e2ee::command::{
    ListMlsCommitsQuery, ListMlsWelcomesQuery, PrincipalDevicesQuery,
};
use rim_services::e2ee::service::E2eeService;

use crate::auth_context::{require_actor, require_mls_caller};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    MlsCommitPage, MlsKeyPackageStatus, MlsWelcome, MlsWelcomeCursorInput, PrincipalDevices,
};

#[derive(Default)]
pub(crate) struct E2eeQueries;

#[Object]
impl E2eeQueries {
    /// The session device's stored key packages, per cipher suite.
    async fn my_mls_key_package_status(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<MlsKeyPackageStatus>> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let status = svc
            .key_package_status(caller)
            .await
            .map_err(map_service_error)?;
        Ok(status.into_iter().map(Into::into).collect())
    }

    /// The session device's unacknowledged Welcomes, oldest first, after `after` (exclusive;
    /// from the oldest when null). `limit` is 1 to 100, 100 by default.
    async fn my_mls_welcomes(
        &self,
        ctx: &Context<'_>,
        after: Option<MlsWelcomeCursorInput>,
        limit: Option<i32>,
    ) -> Result<Vec<MlsWelcome>> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let after = match after {
            Some(cursor) => Some(MlsWelcomeCursor {
                created_at: cursor.created_at,
                id: parse_uuid_id(&cursor.welcome_id)?,
            }),
            None => None,
        };
        let welcomes = svc
            .pending_welcomes(ListMlsWelcomesQuery {
                caller,
                after,
                limit,
            })
            .await
            .map_err(map_service_error)?;
        Ok(welcomes.into_iter().map(Into::into).collect())
    }

    /// Accepted Commits of an incarnation after `afterEpoch` (exclusive; from 0 when null),
    /// oldest first. `limit` is 1 to 100, 50 by default.
    async fn mls_commits(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        incarnation: i32,
        after_epoch: Option<i32>,
        limit: Option<i32>,
    ) -> Result<MlsCommitPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let page = svc
            .list_commits(ListMlsCommitsQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                conversation_id: parse_uuid_id(&conversation_id)?,
                incarnation,
                after_epoch: after_epoch.map(i64::from),
                limit,
            })
            .await
            .map_err(map_service_error)?;
        Ok(page.into())
    }

    /// The certified devices of principals who share a workspace with the caller (the direct
    /// space included); an empty list for any other principal. At most 100 ids.
    async fn principal_devices(
        &self,
        ctx: &Context<'_>,
        principal_ids: Vec<String>,
    ) -> Result<Vec<PrincipalDevices>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        // Remote principals' stored copies are refreshed from their relays when over an hour
        // old; a relay that does not answer leaves the stored copy.
        if let Ok(federation) = ctx.data::<crate::federation::FederationContext>()
            && let Err(e) = federation.host.refresh_remote(principal_ids.clone()).await
        {
            tracing::warn!(error = %e, "could not refresh remote principals");
        }
        let devices = svc
            .principal_devices(PrincipalDevicesQuery {
                actor: Actor::new(actor),
                principal_ids,
            })
            .await
            .map_err(map_service_error)?;
        Ok(devices.into_iter().map(Into::into).collect())
    }
}
