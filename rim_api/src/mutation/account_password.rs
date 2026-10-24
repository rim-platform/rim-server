use async_graphql::*;
use rim_core::account_password::model::{PasswordLoginId, PasswordResetId};
use rim_services::Actor;
use rim_services::account_password::dto::{
    PasswordChangeFinishCmd, PasswordChangeStartCmd, PasswordLoginFinishCmd, PasswordLoginStartCmd,
    PasswordRegistrationStartCmd, PasswordResetChallengeCmd, PasswordResetFinishCmd,
    PasswordResetStartCmd,
};
use rim_services::account_password::service::AccountPasswordService;

use crate::auth_context::{remote_ip, require_actor, require_session_id};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::account_password::{decode_bytes, encode_bytes};
use crate::types::{
    KeyLoginChallenge, PasswordChangeStart, PasswordChanged, PasswordLoginFinish,
    PasswordLoginStart, PasswordResetStart, SignedStatementInput,
};

#[derive(Default)]
pub(crate) struct AccountPasswordMutations;

#[Object]
impl AccountPasswordMutations {
    /// Start registering the password of a new account, before `signupWithKey`: the OPAQUE
    /// `RegistrationRequest` for `principalId`, answered with the `RegistrationResponse`. No
    /// session needed. `CONFLICT` when an account of this server has the principal;
    /// `RATE_LIMITED` when the address asks too often.
    async fn password_registration_start(
        &self,
        ctx: &Context<'_>,
        principal_id: String,
        request: String,
    ) -> Result<String> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.registration_start(PasswordRegistrationStartCmd {
            principal_id,
            request: decode_bytes(&request, "request")?,
            remote_ip: remote_ip(ctx),
        })
        .await
        .map(|response| encode_bytes(&response))
        .map_err(map_service_error)
    }

    /// Start a password login, before `passwordLoginFinish`: the OPAQUE `CredentialRequest` for
    /// `principalId`. Every start counts as a wrong try until a login finishes: tries 1 to 3
    /// are free, the 4th to the 9th make the next one wait (`RATE_LIMITED` with
    /// `retryAfterSeconds`), and the 10th locks the password (`PASSWORD_LOCKED`) until a reset
    /// with the recovery phrase. A principal without a password here gets an answer of the same
    /// shape that no password finishes. No session needed.
    async fn password_login_start(
        &self,
        ctx: &Context<'_>,
        principal_id: String,
        request: String,
    ) -> Result<PasswordLoginStart> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.login_start(PasswordLoginStartCmd {
            principal_id,
            request: decode_bytes(&request, "request")?,
            remote_ip: remote_ip(ctx),
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Finish a password login with the OPAQUE `CredentialFinalization`. On success the tries
    /// are cleared, and the answer carries a single-use ticket for `loginWithKey` (valid for
    /// 120 seconds), the account's server share sealed under the login's transport key, and its
    /// rewraps. The login id is used once, whatever the outcome. No session needed. Every
    /// refusal is `UNAUTHENTICATED`: an unknown, used or expired login, or a wrong password.
    async fn password_login_finish(
        &self,
        ctx: &Context<'_>,
        login_id: ID,
        credential_finalization: String,
    ) -> Result<PasswordLoginFinish> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.login_finish(PasswordLoginFinishCmd {
            login_id: parse_uuid_id::<PasswordLoginId>(&login_id)?,
            credential_finalization: decode_bytes(
                &credential_finalization,
                "credentialFinalization",
            )?,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Start changing the signed-in account's password: the OPAQUE `CredentialRequest` for the
    /// current password and the `RegistrationRequest` for the new one. Counts as a try, as for
    /// `passwordLoginStart`. `NOT_FOUND` when the account has no password.
    async fn password_change_start(
        &self,
        ctx: &Context<'_>,
        credential_request: String,
        registration_request: String,
    ) -> Result<PasswordChangeStart> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.change_start(PasswordChangeStartCmd {
            actor: Actor::new(actor),
            credential_request: decode_bytes(&credential_request, "credentialRequest")?,
            registration_request: decode_bytes(&registration_request, "registrationRequest")?,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Finish a password change: the `CredentialFinalization` for the current password, the
    /// `RegistrationUpload` for the new one, and the rewrap (the previous unlock key sealed
    /// under the new one) for the account's other devices. Keeps the server share, keeps the
    /// newest three rewraps, and ends every other session of the account. `UNAUTHENTICATED`
    /// when the current password does not verify or the login is unknown or expired.
    async fn password_change_finish(
        &self,
        ctx: &Context<'_>,
        login_id: ID,
        credential_finalization: String,
        registration_upload: String,
        rewrap: String,
    ) -> Result<PasswordChanged> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.change_finish(PasswordChangeFinishCmd {
            actor: Actor::new(actor),
            session_id,
            login_id: parse_uuid_id::<PasswordLoginId>(&login_id)?,
            credential_finalization: decode_bytes(
                &credential_finalization,
                "credentialFinalization",
            )?,
            registration_upload: decode_bytes(&registration_upload, "registrationUpload")?,
            rewrap: decode_bytes(&rewrap, "rewrap")?,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Issue a single-use challenge, valid for 120 seconds, for a `password-reset` statement
    /// signed by the recovery key. No session needed.
    async fn password_reset_challenge(
        &self,
        ctx: &Context<'_>,
        principal_id: String,
    ) -> Result<KeyLoginChallenge> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.reset_challenge(PasswordResetChallengeCmd {
            principal_id,
            remote_ip: remote_ip(ctx),
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Start a password reset: a `password-reset` statement over a `passwordResetChallenge`,
    /// signed by the recovery key of the account's key log head, and the
    /// `RegistrationRequest` for the new password. No session needed. `UNAUTHENTICATED` for
    /// any statement that does not verify, deliberately not told apart.
    async fn password_reset_start(
        &self,
        ctx: &Context<'_>,
        statement: SignedStatementInput,
        registration_request: String,
    ) -> Result<PasswordResetStart> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.reset_start(PasswordResetStartCmd {
            statement: statement.into(),
            registration_request: decode_bytes(&registration_request, "registrationRequest")?,
            remote_ip: remote_ip(ctx),
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Finish a password reset with the `RegistrationUpload` for the new password. Keeps the
    /// server share, deletes the rewraps, clears the tries and the lock, and ends every session
    /// of the account. No session needed. `UNAUTHENTICATED` when the reset is unknown, used or
    /// expired. Returns `true`.
    async fn password_reset_finish(
        &self,
        ctx: &Context<'_>,
        reset_id: ID,
        registration_upload: String,
    ) -> Result<bool> {
        let svc = ctx.data::<std::sync::Arc<dyn AccountPasswordService>>()?;
        svc.reset_finish(PasswordResetFinishCmd {
            reset_id: parse_uuid_id::<PasswordResetId>(&reset_id)?,
            registration_upload: decode_bytes(&registration_upload, "registrationUpload")?,
        })
        .await
        .map(|()| true)
        .map_err(map_service_error)
    }
}
