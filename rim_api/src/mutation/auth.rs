use async_graphql::*;
use rim_services::Actor;
use rim_services::auth::command::{LoginCmd, LogoutCmd};
use rim_services::auth::service::AuthService;

use crate::auth_context::{require_actor, require_session_id};
use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::errors::map_service_error;

#[derive(Default)]
pub(crate) struct AuthMutations;

#[Object]
impl AuthMutations {
    async fn login(&self, ctx: &Context<'_>, input: LoginInput) -> Result<LoginPayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        // Remote IP for rate limiting — service enforces the limit.
        let remote_ip = crate::auth_context::remote_ip(ctx);
        let res = auth
            .login(LoginCmd {
                username: input.username,
                password: input.password,
                remote_ip,
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

    async fn logout(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let session_id = require_session_id(ctx)?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.logout(LogoutCmd {
            actor: Actor::new(actor),
            session_id,
        })
        .await
        .map_err(map_service_error)?;

        if let Ok(intent) = ctx.data::<std::sync::Arc<AuthIntentCtx>>() {
            intent.set(AuthIntent::ClearSession);
        }
        Ok(true)
    }
}

#[derive(Default)]
pub(crate) struct PasswordResetMutations;

#[Object]
impl PasswordResetMutations {
    async fn request_password_reset(&self, ctx: &Context<'_>, username: String) -> Result<bool> {
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::password_reset::service::PasswordResetService>>(
            )?;
        svc.request_reset(
            rim_services::password_reset::command::RequestPasswordResetCmd {
                username,
                remote_ip: crate::auth_context::remote_ip(ctx),
            },
        )
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn reset_password(
        &self,
        ctx: &Context<'_>,
        token: String,
        new_password: String,
    ) -> Result<bool> {
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::password_reset::service::PasswordResetService>>(
            )?;
        svc.reset_password(rim_services::password_reset::command::ResetPasswordCmd {
            token,
            new_password,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(SimpleObject)]
pub struct LoginPayload {
    pub user_id: ID,
}
