use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::auth::service::AuthService;
use rim_services::{Actor, ServiceError};

use crate::auth_intent::{AuthIntent, AuthIntentCtx};
use crate::query::AuthActorCtx;

pub struct Mutation;

#[derive(InputObject)]
pub struct BootstrapInput {
    pub secret: String,
    pub username: String,
    pub password: String,
    pub server_name: String,
}

#[derive(SimpleObject)]
pub struct BootstrapPayload {
    pub user_id: ID,
    pub server_id: ID,
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

#[derive(InputObject)]
pub struct CreateInviteInput {
    pub server_id: ID,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
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

#[Object]
impl Mutation {
    async fn bootstrap(
        &self,
        ctx: &Context<'_>,
        input: BootstrapInput,
    ) -> Result<BootstrapPayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .bootstrap(
                &input.secret,
                &input.username,
                &input.password,
                &input.server_name,
            )
            .await
            .map_err(map_service_error)?;

        ctx.data_unchecked::<AuthIntentCtx>()
            .set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });

        Ok(BootstrapPayload {
            user_id: ID(res.user_id.to_string()),
            server_id: ID(res.server_id.to_string()),
        })
    }

    async fn login(&self, ctx: &Context<'_>, input: LoginInput) -> Result<LoginPayload> {
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .login(&input.username, &input.password)
            .await
            .map_err(map_service_error)?;

        ctx.data_unchecked::<AuthIntentCtx>()
            .set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });

        Ok(LoginPayload {
            user_id: ID(res.user_id.to_string()),
        })
    }

    async fn logout(&self, ctx: &Context<'_>) -> Result<bool> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let session_id = ctx
            .data_unchecked::<AuthActorCtx>()
            .session_id()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        auth.logout(Actor::new(actor), session_id)
            .await
            .map_err(map_service_error)?;

        ctx.data_unchecked::<AuthIntentCtx>()
            .set(AuthIntent::ClearSession);
        Ok(true)
    }

    async fn create_invite(
        &self,
        ctx: &Context<'_>,
        input: CreateInviteInput,
    ) -> Result<CreateInvitePayload> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let res = auth
            .create_invite(
                Actor::new(actor),
                server_id,
                input.expires_at,
                input.max_uses.unwrap_or(1),
            )
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
            .accept_invite(&input.invite_token, &input.username, &input.password)
            .await
            .map_err(map_service_error)?;

        ctx.data_unchecked::<AuthIntentCtx>()
            .set(AuthIntent::SetSession {
                session_token: res.session_token,
                csrf_token: res.csrf_token,
            });

        Ok(AcceptInvitePayload {
            user_id: ID(res.user_id.to_string()),
            server_id: ID(res.server_id.to_string()),
        })
    }
}

fn map_service_error(err: ServiceError) -> Error {
    match err {
        ServiceError::PermissionDenied => Error::new("permission denied"),
        ServiceError::Validation(m) => Error::new(m),
        ServiceError::Conflict(m) => Error::new(m),
        ServiceError::NotFound => Error::new("not found"),
        _ => Error::new("internal error"),
    }
}

fn parse_uuid_id<T>(id: &ID) -> Result<T>
where
    T: From<uuid::Uuid>,
{
    let uuid = uuid::Uuid::parse_str(id.as_str()).map_err(|e| Error::new(e.to_string()))?;
    Ok(uuid.into())
}
