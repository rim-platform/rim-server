use async_graphql::*;
use rim_services::Actor;
use rim_services::auth::dto::RegistrationMode;
use rim_services::key_auth::command::{
    AddDeviceKeyCmd, EnrollIdentityKeyCmd, IssueKeyChallengeCmd, LoginWithKeyCmd,
    PublishProfileCardCmd, RevokeDeviceKeyCmd, RotateIdentityKeyCmd, SignupWithKeyCmd,
};
use rim_services::key_auth::service::KeyAuthService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::mutation::auth::LoginPayload;
use crate::mutation::invite::registration_token;
use crate::types::{
    DeviceKey, EnrollIdentityKeyInput, KeyIdentity, KeyLoginChallenge, ProfileCard,
    RotateIdentityKeyInput, SignedStatementInput, SignupWithInvitePayloadGql, SignupWithKeyInput,
};

#[derive(Default)]
pub(crate) struct KeyIdentityMutations;

#[Object]
impl KeyIdentityMutations {
    /// Start a key login: a single-use challenge for a device to sign. No session needed.
    async fn key_login_challenge(&self, ctx: &Context<'_>) -> Result<KeyLoginChallenge> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .issue_challenge(IssueKeyChallengeCmd {
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Sign in with a `login` statement signed by an enrolled device key. Sets the same session
    /// cookie as `login`.
    async fn login_with_key(
        &self,
        ctx: &Context<'_>,
        proof: SignedStatementInput,
    ) -> Result<LoginPayload> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let res = svc
            .login_with_key(LoginWithKeyCmd {
                proof: proof.into(),
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(LoginPayload {
            user_id: ID(res.user_id.to_string()),
        })
    }

    /// Bind a key identity to the signed-in account. Afterwards the password no longer signs in.
    async fn enroll_identity_key(
        &self,
        ctx: &Context<'_>,
        input: EnrollIdentityKeyInput,
    ) -> Result<KeyIdentity> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .enroll(EnrollIdentityKeyCmd {
                actor: Actor::new(actor),
                session_id,
                key_log: input.key_log.into_iter().map(Into::into).collect(),
                device: input.device.into(),
                proof: input.proof.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Add a device certified by the current identity key.
    async fn add_device_key(
        &self,
        ctx: &Context<'_>,
        certificate: SignedStatementInput,
    ) -> Result<DeviceKey> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .add_device(AddDeviceKeyCmd {
                actor: Actor::new(actor),
                device: certificate.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Revoke a device, signed by the identity key or by the device itself. Ends its sessions.
    async fn revoke_device_key(
        &self,
        ctx: &Context<'_>,
        revocation: SignedStatementInput,
    ) -> Result<DeviceKey> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .revoke_device(RevokeDeviceKeyCmd {
                actor: Actor::new(actor),
                revocation: revocation.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Append a rotation to the key log. A rotation signed by the recovery key works without a
    /// session, revokes every device and session, and enrolls the device given with it.
    async fn rotate_identity_key(
        &self,
        ctx: &Context<'_>,
        input: RotateIdentityKeyInput,
    ) -> Result<KeyIdentity> {
        let actor = crate::auth_context::optional_actor(ctx);
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .rotate(RotateIdentityKeyCmd {
                actor: actor.map(Actor::new),
                entry: input.entry.into(),
                device: input.device.map(Into::into),
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Publish a profile card signed by the current identity key.
    async fn publish_profile_card(
        &self,
        ctx: &Context<'_>,
        card: SignedStatementInput,
    ) -> Result<ProfileCard> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let dto = svc
            .publish_profile_card(PublishProfileCardCmd {
                actor: Actor::new(actor),
                card: card.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Create an account from a key identity alone: no password, no email. The registration
    /// mode applies as for `signupWithInvite` (exactly one token in invite-only mode, none in
    /// open mode). Signs the new account in on the device that signed up, setting the same
    /// session and CSRF cookies as `signupWithInvite`.
    async fn signup_with_key(
        &self,
        ctx: &Context<'_>,
        input: SignupWithKeyInput,
    ) -> Result<SignupWithInvitePayloadGql> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let mode = ctx.data::<std::sync::Arc<RegistrationMode>>()?;
        let token = registration_token(
            mode,
            input.bootstrap_token,
            input.instance_invite_token,
            input.workspace_invite_token,
        )?;
        let res = svc
            .signup_with_key(SignupWithKeyCmd {
                username: input.username,
                display_name: input.display_name,
                key_log: input.key_log.into_iter().map(Into::into).collect(),
                device: input.device.into(),
                proof: input.proof.into(),
                registration_mode: mode.as_ref().clone(),
                token,
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(SignupWithInvitePayloadGql {
            user_id: ID(res.user_id.to_string()),
            server_id: res.server_id.map(|id| ID(id.to_string())),
        })
    }
}
