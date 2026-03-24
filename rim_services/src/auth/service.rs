use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInviteCmd, JoinServerByInviteCmd, LoginCmd, LogoutCmd,
};
use crate::auth::dto::{
    AcceptInviteResultDto, BootstrapResultDto, CreateInviteResultDto, JoinServerByInviteResultDto,
    LoginResultDto, RequestAuthResultDto,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AuthService: Send + Sync {
    async fn bootstrap(&self, cmd: BootstrapCmd) -> ServiceResult<BootstrapResultDto>;

    async fn login(&self, cmd: LoginCmd) -> ServiceResult<LoginResultDto>;

    async fn logout(&self, cmd: LogoutCmd) -> ServiceResult<()>;

    async fn create_invite(&self, cmd: CreateInviteCmd) -> ServiceResult<CreateInviteResultDto>;

    async fn accept_invite(&self, cmd: AcceptInviteCmd) -> ServiceResult<AcceptInviteResultDto>;

    async fn join_server_by_invite(
        &self,
        cmd: JoinServerByInviteCmd,
    ) -> ServiceResult<JoinServerByInviteResultDto>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RequestAuthenticator: Send + Sync {
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
    ) -> ServiceResult<RequestAuthResultDto>;
}
