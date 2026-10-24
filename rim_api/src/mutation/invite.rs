use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    CreateInviteCmd, JoinServerByInviteCmd, RevokeInviteCmd, SignupToken,
};
use rim_services::auth::dto::RegistrationMode;
use rim_services::auth::service::AuthService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::WorkspaceInvite;

#[derive(Default)]
pub(crate) struct InviteMutations;

/// The registration token `signupWithKey` presents, under the deployment's mode: exactly one of
/// the two in invite-only mode, at most one in pow mode, none in open mode. In pow mode a
/// proof-of-work may stand in for the token; `key_signup_credential` checks that.
///
/// # Errors
/// `VALIDATION` when the number of tokens does not fit the mode.
pub(crate) fn registration_token(
    mode: &RegistrationMode,
    instance_invite_token: Option<String>,
    workspace_invite_token: Option<String>,
) -> Result<Option<SignupToken>> {
    let tokens: Vec<SignupToken> = [
        instance_invite_token.map(SignupToken::InstanceInvite),
        workspace_invite_token.map(SignupToken::WorkspaceInvite),
    ]
    .into_iter()
    .flatten()
    .collect();
    let message = match mode {
        RegistrationMode::InviteOnly if tokens.len() != 1 => {
            "exactly one token is required in invite-only mode"
        }
        RegistrationMode::Pow { .. } if tokens.len() > 1 => {
            "at most one token is allowed in pow registration mode"
        }
        RegistrationMode::Open if !tokens.is_empty() => {
            "tokens must not be supplied in open registration mode"
        }
        _ => return Ok(tokens.into_iter().next()),
    };
    Err(async_graphql::Error::new(message).extend_with(|_, e| e.set("code", "VALIDATION")))
}

#[Object]
impl InviteMutations {
    async fn create_invite(
        &self,
        ctx: &Context<'_>,
        input: CreateInviteInput,
    ) -> Result<CreateInvitePayload> {
        let actor = require_actor(ctx)?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor),
                server_id,
                expires_at: input.expires_at,
                max_uses: input.max_uses.unwrap_or(1),
                recipient_email: input.recipient_email,
                role: input.role.map_or(
                    rim_core::server_member::model::ServerMemberRole::Member,
                    Into::into,
                ),
            })
            .await
            .map_err(map_service_error)?;

        Ok(CreateInvitePayload {
            invite_token: res.invite_token,
            invite_id: ID(res.invite.id.to_string()),
        })
    }

    async fn join_server_by_invite(
        &self,
        ctx: &Context<'_>,
        input: JoinServerByInviteInput,
    ) -> Result<JoinServerByInvitePayload> {
        let actor = require_actor(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .join_server_by_invite(JoinServerByInviteCmd {
                actor: Actor::new(actor),
                invite_token: input.invite_token,
            })
            .await
            .map_err(map_service_error)?;

        Ok(JoinServerByInvitePayload {
            server_id: ID(res.server_id.to_string()),
        })
    }
}

#[derive(Default)]
pub(crate) struct WorkspaceInviteMutations;

#[Object]
impl WorkspaceInviteMutations {
    /// Revoke a workspace invite: from now on it admits nobody, and members who joined with it
    /// stay. Owner of the invite's workspace only (`FORBIDDEN` for its other members);
    /// `NOT_FOUND` for an unknown invite or one of a workspace the caller is not in. Revoking a
    /// revoked invite returns it unchanged.
    async fn revoke_invite(&self, ctx: &Context<'_>, invite_id: ID) -> Result<WorkspaceInvite> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let invite = auth
            .revoke_invite(RevokeInviteCmd {
                actor: Actor::new(actor),
                invite_id: parse_uuid_id(&invite_id)?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(invite.into())
    }
}

#[derive(InputObject)]
pub struct CreateInviteInput {
    pub server_id: ID,
    /// When the invite stops admitting anyone: in the future and at most 30 days from now
    /// (`VALIDATION` otherwise). Seven days from now when omitted.
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
    /// Optional recipient to email the invite link to. Validated server-side;
    /// malformed addresses return a validation error.
    pub recipient_email: Option<String>,
    /// Role granted on join: `MEMBER` (default) or `GUEST`. Any other value is a
    /// `VALIDATION` error.
    pub role: Option<crate::types::ServerMemberRoleGql>,
}

#[derive(SimpleObject)]
pub struct CreateInvitePayload {
    pub invite_token: String,
    pub invite_id: ID,
}

#[derive(InputObject)]
pub struct JoinServerByInviteInput {
    pub invite_token: String,
}

#[derive(SimpleObject)]
pub struct JoinServerByInvitePayload {
    pub server_id: ID,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(err: &async_graphql::Error) -> Option<String> {
        match err.extensions.as_ref()?.get("code")? {
            async_graphql::Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    #[test]
    fn invite_only_mode_takes_exactly_one_token() {
        let mode = RegistrationMode::InviteOnly;
        assert_eq!(
            registration_token(&mode, Some("i".into()), None).unwrap(),
            Some(SignupToken::InstanceInvite("i".into()))
        );
        assert_eq!(
            registration_token(&mode, None, Some("w".into())).unwrap(),
            Some(SignupToken::WorkspaceInvite("w".into()))
        );
        for err in [
            registration_token(&mode, None, None).unwrap_err(),
            registration_token(&mode, Some("i".into()), Some("w".into())).unwrap_err(),
        ] {
            assert_eq!(code(&err).as_deref(), Some("VALIDATION"));
            assert!(err.message.contains("exactly one token"));
        }
    }

    #[test]
    fn pow_mode_takes_at_most_one_token() {
        let mode = RegistrationMode::Pow { bits: 24 };
        assert_eq!(
            registration_token(&mode, Some("i".into()), None).unwrap(),
            Some(SignupToken::InstanceInvite("i".into()))
        );
        let err = registration_token(&mode, Some("i".into()), Some("w".into())).unwrap_err();
        assert_eq!(code(&err).as_deref(), Some("VALIDATION"));
        assert!(err.message.contains("at most one token"));
    }

    #[test]
    fn open_mode_takes_no_token() {
        let mode = RegistrationMode::Open;
        assert_eq!(registration_token(&mode, None, None).unwrap(), None);
        let err = registration_token(&mode, None, Some("w".into())).unwrap_err();
        assert_eq!(code(&err).as_deref(), Some("VALIDATION"));
        assert!(err.message.contains("must not be supplied"));
    }
}
