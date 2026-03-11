use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::auth::dto::{
    AcceptInviteResultDto, BootstrapResultDto, CreateInviteResultDto, LoginResultDto,
    RequestAuthResultDto,
};
use crate::{Actor, ServiceResult};
use rim_core::server::model::ServerId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AuthService: Send + Sync {
    async fn bootstrap(
        &self,
        secret: &str,
        username: &str,
        password: &str,
        server_name: &str,
    ) -> ServiceResult<BootstrapResultDto>;

    async fn login(&self, username: &str, password: &str) -> ServiceResult<LoginResultDto>;

    async fn logout(
        &self,
        actor: Actor,
        session_id: rim_core::auth::model::SessionId,
    ) -> ServiceResult<()>;

    async fn create_invite(
        &self,
        actor: Actor,
        server_id: ServerId,
        expires_at: Option<DateTime<Utc>>,
        max_uses: i32,
    ) -> ServiceResult<CreateInviteResultDto>;

    async fn accept_invite(
        &self,
        invite_token: &str,
        username: &str,
        password: &str,
    ) -> ServiceResult<AcceptInviteResultDto>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RequestAuthenticator: Send + Sync {
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
    ) -> ServiceResult<RequestAuthResultDto>;
}
