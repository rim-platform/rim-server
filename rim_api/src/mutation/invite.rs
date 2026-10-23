use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    AcceptInviteCmd, CreateInviteCmd, JoinServerByInviteCmd, RevokeInviteCmd, SignupOpenCmd,
    SignupToken, SignupWithInstanceInviteCmd,
};
use rim_services::auth::dto::RegistrationMode;
use rim_services::auth::service::AuthService;

use crate::auth_context::require_actor;
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{SignupWithInvitePayloadGql, WorkspaceInvite};

#[derive(Default)]
pub(crate) struct InviteMutations;

/// The registration token a signup presents, under the deployment's mode: exactly one of the
/// two in invite-only and pow mode, none in open mode. Shared by `signupWithInvite` and
/// `signupWithKey` (which in pow mode may give a proof-of-work instead, see
/// `key_signup_credential`).
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
        RegistrationMode::Pow { .. } if tokens.len() != 1 => {
            "exactly one token is required for a password signup in pow registration mode"
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

    /// Unified signup mutation for all token-based and open-mode registration paths.
    ///
    /// In invite-only mode, exactly one of `instanceInviteToken` or `workspaceInviteToken`
    /// must be supplied.
    /// In open mode, all token fields must be absent.
    /// Refused with `FORBIDDEN` when `serverInfo.signupRequiresKeys` is true: new accounts then
    /// sign up with `signupWithKey`.
    ///
    /// On success, sets session and CSRF cookies via `AuthIntent`.
    async fn signup_with_invite(
        &self,
        ctx: &Context<'_>,
        input: SignupWithInviteInput,
    ) -> Result<SignupWithInvitePayloadGql> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let mode = ctx.data::<std::sync::Arc<RegistrationMode>>()?;
        auth.check_registration_capacity()
            .await
            .map_err(map_service_error)?;
        let token = registration_token(
            mode,
            input.instance_invite_token,
            input.workspace_invite_token,
        )?;
        let terms_accepted_url = input.accepted_terms_url;
        let confirmed_minimum_age = input.confirmed_minimum_age;

        // Normalise email at the resolver boundary (trim + lowercase) as
        // defence in depth; the service also validates on the way in.
        let email = input.email.trim().to_lowercase();

        let (user_id, server_id, session_token, csrf_token) = match token {
            Some(SignupToken::InstanceInvite(raw)) => {
                let res = auth
                    .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                        instance_invite_token: raw,
                        username: input.username,
                        password: input.password,
                        email,
                        display_name: input.display_name,
                        terms_accepted_url,
                        confirmed_minimum_age,
                    })
                    .await
                    .map_err(map_service_error)?;
                (
                    res.user_id,
                    res.server_id,
                    res.session_token,
                    res.csrf_token,
                )
            }
            Some(SignupToken::WorkspaceInvite(raw)) => {
                // Delegate to the existing accept_invite path.
                let res = auth
                    .accept_invite(AcceptInviteCmd {
                        invite_token: raw,
                        username: input.username,
                        password: input.password,
                        terms_accepted_url,
                        confirmed_minimum_age,
                    })
                    .await
                    .map_err(map_service_error)?;
                (
                    res.user_id,
                    Some(res.server_id),
                    res.session_token,
                    res.csrf_token,
                )
            }
            None => {
                // Open mode. Remote IP for rate limiting — service enforces the limit.
                let remote_ip = crate::auth_context::remote_ip(ctx);

                let res = auth
                    .signup_open(SignupOpenCmd {
                        username: input.username,
                        password: input.password,
                        remote_ip,
                        email,
                        display_name: input.display_name,
                        terms_accepted_url,
                        confirmed_minimum_age,
                    })
                    .await
                    .map_err(map_service_error)?;
                (
                    res.user_id,
                    res.server_id,
                    res.session_token,
                    res.csrf_token,
                )
            }
        };

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token,
                csrf_token: csrf_token.clone(),
            });
        }

        Ok(SignupWithInvitePayloadGql {
            user_id: ID(user_id.to_string()),
            server_id: server_id.map(|id| ID(id.to_string())),
            csrf_token,
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

// ── Registration inputs ───────────────────────────────────────────────────

/// Input for `signupWithInvite`.
///
/// Exactly one token MUST be set in invite-only mode; all must be absent in
/// open mode. The resolver enforces this with a `Validation` error.
///
/// `email` is required (max 254 chars, server trims + lowercases, case-insensitive unique).
/// `displayName` is optional; falls back to `username` server-side if absent or blank.
#[derive(InputObject)]
pub struct SignupWithInviteInput {
    pub username: String,
    /// Required. The user's email address. Trimmed and lowercased before storage.
    /// Must look like `local@domain.tld` and be at most 254 characters.
    pub email: String,
    /// Optional display name, stored without control and invisible format characters and with
    /// whitespace collapsed; at most 64 characters after that. Falls back to `username` when
    /// absent or when nothing visible is left.
    pub display_name: Option<String>,
    pub password: String,
    pub instance_invite_token: Option<String>,
    pub workspace_invite_token: Option<String>,
    /// The terms of service the person accepted: `serverInfo.termsUrl`, sent back unchanged.
    /// Required when the server has terms (`VALIDATION` when missing or different); ignored
    /// when it has none.
    pub accepted_terms_url: Option<String>,
    /// The minimum age the person confirmed reaching: `serverInfo.minimumAge`, sent back
    /// unchanged once they confirm it. Required when the server has a minimum age
    /// (`VALIDATION` when missing or different); ignored when it has none.
    pub confirmed_minimum_age: Option<i32>,
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
    fn pow_mode_takes_one_token_for_a_password_signup() {
        let mode = RegistrationMode::Pow { bits: 24 };
        assert_eq!(
            registration_token(&mode, Some("i".into()), None).unwrap(),
            Some(SignupToken::InstanceInvite("i".into()))
        );
        let err = registration_token(&mode, None, None).unwrap_err();
        assert_eq!(code(&err).as_deref(), Some("VALIDATION"));
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
