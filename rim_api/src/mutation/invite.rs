use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::auth::command::{
    AcceptInviteCmd, CreateInstanceInviteCmd, CreateInviteCmd, JoinServerByInviteCmd,
    RevokeInstanceInviteCmd, RevokeInviteCmd, SignupOpenCmd, SignupWithBootstrapCmd,
    SignupWithInstanceInviteCmd,
};
use rim_services::auth::service::AuthService;

use crate::auth_context::require_actor;
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{CreateInstanceInvitePayloadGql, InstanceInviteGql, SignupWithInvitePayloadGql};

#[derive(Default)]
pub(crate) struct InviteMutations;

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

    async fn accept_invite(
        &self,
        ctx: &Context<'_>,
        input: AcceptInviteInput,
    ) -> Result<AcceptInvitePayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .accept_invite(AcceptInviteCmd {
                invite_token: input.invite_token,
                username: input.username,
                password: input.password,
            })
            .await
            .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });
        }

        Ok(AcceptInvitePayload {
            user_id: ID(res.user_id.to_string()),
            server_id: ID(res.server_id.to_string()),
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

    async fn revoke_invite(&self, ctx: &Context<'_>, invite_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let invite_id = parse_uuid_id::<rim_core::auth::model::InviteId>(&invite_id)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.revoke_invite(RevokeInviteCmd {
            actor: Actor::new(actor),
            invite_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    /// Unified signup mutation for all token-based and open-mode registration paths.
    ///
    /// In invite-only mode, exactly one of `bootstrapToken`, `instanceInviteToken`,
    /// or `workspaceInviteToken` must be supplied.
    /// In open mode, all token fields must be absent.
    ///
    /// On success, sets session and CSRF cookies via `AuthIntent`.
    async fn signup_with_invite(
        &self,
        ctx: &Context<'_>,
        input: SignupWithInviteInput,
    ) -> Result<SignupWithInvitePayloadGql> {
        use rim_services::auth::dto::RegistrationMode;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let mode = ctx.data::<std::sync::Arc<RegistrationMode>>()?;

        let token_count = input.bootstrap_token.is_some() as u8
            + input.instance_invite_token.is_some() as u8
            + input.workspace_invite_token.is_some() as u8;

        let (user_id, server_id, session_token, csrf_token) = match mode.as_ref() {
            RegistrationMode::InviteOnly => {
                if token_count != 1 {
                    return Err(async_graphql::Error::new(
                        "exactly one token is required in invite-only mode",
                    )
                    .extend_with(|_, e| e.set("code", "VALIDATION")));
                }

                // Normalise email at the resolver boundary (trim + lowercase) as
                // defence in depth; the service also validates on the way in.
                let normalised_email = input.email.trim().to_lowercase();

                if let Some(raw) = input.bootstrap_token {
                    let res = auth
                        .signup_with_bootstrap(SignupWithBootstrapCmd {
                            bootstrap_token: raw,
                            username: input.username,
                            password: input.password,
                            email: normalised_email,
                            display_name: input.display_name,
                        })
                        .await
                        .map_err(map_service_error)?;
                    (
                        res.user_id,
                        res.server_id,
                        res.session_token,
                        res.csrf_token,
                    )
                } else if let Some(raw) = input.instance_invite_token {
                    let res = auth
                        .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                            instance_invite_token: raw,
                            username: input.username,
                            password: input.password,
                            email: normalised_email,
                            display_name: input.display_name,
                        })
                        .await
                        .map_err(map_service_error)?;
                    (
                        res.user_id,
                        res.server_id,
                        res.session_token,
                        res.csrf_token,
                    )
                } else {
                    // workspace_invite_token — delegate to the existing accept_invite path.
                    let raw = input.workspace_invite_token.unwrap();
                    let res = auth
                        .accept_invite(AcceptInviteCmd {
                            invite_token: raw,
                            username: input.username,
                            password: input.password,
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
            }
            RegistrationMode::Open => {
                if token_count > 0 {
                    return Err(async_graphql::Error::new(
                        "tokens must not be supplied in open registration mode",
                    )
                    .extend_with(|_, e| e.set("code", "VALIDATION")));
                }

                // Proof-of-wiring: fail closed if signup rate limiter token is absent.
                let _ =
                    ctx.data::<std::sync::Arc<rim_services::auth::dto::SignupRateLimiterToken>>();

                // Remote IP for rate limiting — service enforces the limit.
                let remote_ip = crate::auth_context::remote_ip(ctx);

                let normalised_email_open = input.email.trim().to_lowercase();
                let res = auth
                    .signup_open(SignupOpenCmd {
                        username: input.username,
                        password: input.password,
                        remote_ip,
                        email: normalised_email_open,
                        display_name: input.display_name,
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
                csrf_token,
            });
        }

        Ok(SignupWithInvitePayloadGql {
            user_id: ID(user_id.to_string()),
            server_id: server_id.map(|id| ID(id.to_string())),
        })
    }

    /// Create a new instance invite (admin-only gate: must own at least one workspace).
    ///
    /// Returns the raw invite token exactly once in the payload. Store it; it cannot
    /// be retrieved again.
    async fn create_instance_invite(
        &self,
        ctx: &Context<'_>,
        input: CreateInstanceInviteInput,
    ) -> Result<CreateInstanceInvitePayloadGql> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let res = auth
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor),
                expires_at: input.expires_at,
                max_uses: input.max_uses,
                recipient_email: input.recipient_email,
            })
            .await
            .map_err(map_service_error)?;

        Ok(CreateInstanceInvitePayloadGql {
            invite_token: res.invite_token,
            invite: InstanceInviteGql::from(res.invite),
        })
    }

    /// Revoke an existing instance invite (admin-only gate: must own at least one workspace).
    ///
    /// Returns `true` on success. Returns `NOT_FOUND` if the invite does not exist.
    async fn revoke_instance_invite(&self, ctx: &Context<'_>, invite_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let invite_id = parse_uuid_id::<rim_core::auth::model::InstanceInviteId>(&invite_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        auth.revoke_instance_invite(RevokeInstanceInviteCmd {
            actor: Actor::new(actor),
            invite_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct CreateInviteInput {
    pub server_id: ID,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
    /// Optional recipient to email the invite link to (Track B3). Validated server-side;
    /// malformed addresses return a validation error.
    pub recipient_email: Option<String>,
    /// Role granted on join (Track 1.2): `MEMBER` (default) or `GUEST`. Any other value is a
    /// `VALIDATION` error.
    pub role: Option<crate::types::ServerMemberRoleGql>,
}

#[derive(SimpleObject)]
pub struct CreateInvitePayload {
    pub invite_token: String,
    pub invite_id: ID,
}

#[derive(InputObject)]
pub struct AcceptInviteInput {
    pub invite_token: String,
    pub username: String,
    pub password: String,
}

#[derive(SimpleObject)]
pub struct AcceptInvitePayload {
    pub user_id: ID,
    pub server_id: ID,
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
    /// Must match the pragmatic email regex from spec §13.3.
    pub email: String,
    /// Optional display name (1–80 chars when present). Falls back to `username`
    /// when absent or blank after trim.
    pub display_name: Option<String>,
    pub password: String,
    pub bootstrap_token: Option<String>,
    pub instance_invite_token: Option<String>,
    pub workspace_invite_token: Option<String>,
}

/// Input for `createInstanceInvite`.
///
/// Both fields are optional: `expires_at` defaults to `now() + 7 days`,
/// `max_uses` defaults to 1.
#[derive(InputObject)]
pub struct CreateInstanceInviteInput {
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_uses: Option<i32>,
    /// Optional recipient to email the invite link to (Track B3). Validated server-side;
    /// malformed addresses return a validation error.
    pub recipient_email: Option<String>,
}
