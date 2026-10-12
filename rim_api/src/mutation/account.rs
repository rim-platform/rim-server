use async_graphql::*;
use rim_services::Actor;
use rim_services::account_deletion::dto::RequestAccountDeletionCmd;
use rim_services::account_deletion::service::AccountDeletionService;

use crate::auth_context::{require_actor, require_session_id};
use crate::errors::map_service_error;
use crate::types::{AccountDeletionSchedule, RequestAccountDeletionInput};

#[derive(Default)]
pub(crate) struct AccountMutations;

#[Object]
impl AccountMutations {
    /// Schedule the signed-in account for deletion once the grace period ends (14 days unless
    /// the operator set `RIM_ACCOUNT_DELETION_GRACE_DAYS`, 0 to 30). Every other session of the
    /// account ends now; this one keeps working, and `cancelAccountDeletion` keeps the account
    /// until the grace period ends. `me.deletionScheduledFor` shows the pending deletion.
    ///
    /// Then the account is purged: its sessions, devices, keys, workspace memberships,
    /// plaintext messages, files and personal settings are removed, workspaces where it was the
    /// only member are deleted, and its username becomes free. The user row stays as a
    /// tombstone (`deleted-<id prefix>`, "Deleted account", no email, password or key) that
    /// cannot sign in and shows in no member list or search; it stays the author of the
    /// account's encrypted messages, which are kept so the conversations' history does not
    /// shift, and of a deleted message others replied to. The tombstone keeps the account's
    /// key identity id, so the same key identity cannot sign up on this server again.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED`: no session.
    /// - `FORBIDDEN`: a person from another server, who deletes their account at their relay.
    /// - `VALIDATION`: `confirmUsername` is not the account's username, or the account owns a
    ///   workspace that has other members: hand it over (`transferWorkspaceOwnership`) or delete
    ///   it (`deleteWorkspace`) first.
    /// - `CONFLICT`: a deletion is already pending.
    async fn request_account_deletion(
        &self,
        ctx: &Context<'_>,
        input: RequestAccountDeletionInput,
    ) -> Result<AccountDeletionSchedule> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AccountDeletionService>>()?;
        svc.request_deletion(RequestAccountDeletionCmd {
            actor: Actor::new(actor),
            session_id,
            confirm_username: input.confirm_username,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Cancel the signed-in account's pending deletion during its grace period. Returns whether
    /// a deletion was pending.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED`: no session.
    async fn cancel_account_deletion(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AccountDeletionService>>()?;
        svc.cancel_deletion(Actor::new(actor))
            .await
            .map_err(map_service_error)
    }
}
