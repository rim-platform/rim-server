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
use crate::types::account_password::{decode_bytes, encode_bytes};
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
    ///
    /// An account of this server with a password also brings `passwordTicket`, from
    /// `passwordLoginFinish`: single use, valid for 120 seconds, and only for that account.
    /// Without one, or with one that is used, expired or another account's, the answer is
    /// `UNAUTHENTICATED`, as for a wrong proof. An account of this server without a password
    /// signs in with the key alone and gets `mustSetPassword`: its session may only set a
    /// password, sign out and read `me`. Accounts of other servers sign in with the key alone.
    async fn login_with_key(
        &self,
        ctx: &Context<'_>,
        proof: SignedStatementInput,
        password_ticket: Option<String>,
    ) -> Result<LoginPayload> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let res = svc
            .login_with_key(LoginWithKeyCmd {
                proof: proof.into(),
                password_ticket,
                remote_ip: crate::auth_context::remote_ip(ctx),
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token.clone(),
            });
        }

        Ok(LoginPayload {
            user_id: ID(res.user_id.to_string()),
            csrf_token: res.csrf_token,
            must_set_password: res.must_set_password,
        })
    }

    /// Bind a key identity to the signed-in account. The calling session becomes the new
    /// device's and every other session of the account ends. Afterwards the password no longer
    /// signs in.
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

    /// Create an account from a key identity and a password: `passwordRegistration` is the
    /// OPAQUE `RegistrationUpload` registered with `passwordRegistrationStart`. The registration mode
    /// applies as for `signupWithInvite` (exactly one token in invite-only mode, none in open
    /// mode). An instance invite bound to an email address gives the account that address
    /// (`EMAIL_TAKEN` when another account has it); otherwise the account has no email. Signs
    /// the new account in on the device that signed up, setting the same session and CSRF
    /// cookies as `signupWithInvite`, and returns the account's new server share in the clear.
    async fn signup_with_key(
        &self,
        ctx: &Context<'_>,
        input: SignupWithKeyInput,
    ) -> Result<SignupWithInvitePayloadGql> {
        let svc = ctx.data::<std::sync::Arc<dyn KeyAuthService>>()?;
        let mode = ctx.data::<std::sync::Arc<RegistrationMode>>()?;
        let auth = ctx.data::<std::sync::Arc<dyn rim_services::auth::service::AuthService>>()?;
        auth.check_registration_capacity()
            .await
            .map_err(map_service_error)?;
        let (token, proof_of_work) = key_signup_credential(
            mode,
            input.instance_invite_token,
            input.workspace_invite_token,
            input.proof_of_work,
        )?;
        let password_registration =
            decode_bytes(&input.password_registration, "passwordRegistration")?;
        let res = svc
            .signup_with_key(SignupWithKeyCmd {
                username: input.username,
                display_name: input.display_name,
                key_log: input.key_log.into_iter().map(Into::into).collect(),
                device: input.device.into(),
                proof: input.proof.into(),
                password_registration,
                registration_mode: mode.as_ref().clone(),
                token,
                proof_of_work,
                remote_ip: crate::auth_context::remote_ip(ctx),
                terms_accepted_url: input.accepted_terms_url,
                confirmed_minimum_age: input.confirmed_minimum_age,
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token.clone(),
            });
        }

        Ok(SignupWithInvitePayloadGql {
            user_id: ID(res.user_id.to_string()),
            server_id: res.server_id.map(|id| ID(id.to_string())),
            csrf_token: res.csrf_token,
            server_share: res.server_share.as_deref().map(encode_bytes),
        })
    }
}

/// What a key-only signup presents under the deployment's mode: in pow mode exactly one of a
/// registration token and a proof-of-work; otherwise the token rules of
/// [`registration_token`], with no proof-of-work.
///
/// # Errors
/// `VALIDATION` when what is given does not fit the mode.
fn key_signup_credential(
    mode: &RegistrationMode,
    instance_invite_token: Option<String>,
    workspace_invite_token: Option<String>,
    proof_of_work: Option<String>,
) -> Result<(
    Option<rim_services::auth::command::SignupToken>,
    Option<String>,
)> {
    let invalid =
        |message: &str| Err(Error::new(message).extend_with(|_, e| e.set("code", "VALIDATION")));
    let any_token = instance_invite_token.is_some() || workspace_invite_token.is_some();
    match (mode, proof_of_work) {
        (RegistrationMode::Pow { .. }, Some(_)) if any_token => {
            invalid("give a registration token or proofOfWork, not both")
        }
        (RegistrationMode::Pow { .. }, Some(nonce)) => Ok((None, Some(nonce))),
        (RegistrationMode::Pow { .. }, None) if !any_token => {
            invalid("a registration token or proofOfWork is required")
        }
        (_, Some(_)) => invalid("proofOfWork is only taken in pow registration mode"),
        (_, None) => Ok((
            registration_token(mode, instance_invite_token, workspace_invite_token)?,
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use rim_services::auth::command::SignupToken;

    use super::*;

    fn message(result: Result<(Option<SignupToken>, Option<String>)>) -> String {
        result.expect_err("refused").message
    }

    #[test]
    fn pow_mode_takes_a_token_or_a_proof_of_work() {
        let mode = RegistrationMode::Pow { bits: 24 };
        assert_eq!(
            key_signup_credential(&mode, None, None, Some("AAAAAAAAkaM".into())).unwrap(),
            (None, Some("AAAAAAAAkaM".into()))
        );
        assert_eq!(
            key_signup_credential(&mode, Some("i".into()), None, None).unwrap(),
            (Some(SignupToken::InstanceInvite("i".into())), None)
        );
        assert!(
            message(key_signup_credential(
                &mode,
                Some("i".into()),
                None,
                Some("n".into())
            ))
            .contains("not both")
        );
        assert!(message(key_signup_credential(&mode, None, None, None)).contains("required"));
    }

    #[test]
    fn other_modes_refuse_a_proof_of_work() {
        for mode in [RegistrationMode::Open, RegistrationMode::InviteOnly] {
            assert!(
                message(key_signup_credential(&mode, None, None, Some("n".into())))
                    .contains("pow registration mode")
            );
        }
        assert_eq!(
            key_signup_credential(&RegistrationMode::Open, None, None, None).unwrap(),
            (None, None)
        );
    }
}
