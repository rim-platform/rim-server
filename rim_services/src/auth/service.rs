use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::auth::command::{
    AcceptInviteCmd, CreateInviteCmd, JoinServerByInviteCmd, LoginCmd, LogoutCmd,
    LogoutOtherSessionsCmd, PreviewInviteTokenQuery, SignupOpenCmd, SignupWithInstanceInviteCmd,
};
use crate::auth::dto::{
    AcceptInviteResultDto, CreateInviteResultDto, InviteTokenPreviewDto,
    JoinServerByInviteResultDto, LoginResultDto, RequestAuthResultDto, SignupResultDto,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AuthService: Send + Sync {
    async fn login(&self, cmd: LoginCmd) -> ServiceResult<LoginResultDto>;

    async fn logout(&self, cmd: LogoutCmd) -> ServiceResult<()>;

    /// Revoke every session of the actor's account except the calling one. Returns how many
    /// were revoked.
    ///
    /// # Errors
    /// - `NotFound` — the calling session does not exist.
    /// - `PermissionDenied` — the calling session belongs to another account.
    async fn logout_other_sessions(&self, cmd: LogoutOtherSessionsCmd) -> ServiceResult<u64>;

    async fn create_invite(&self, cmd: CreateInviteCmd) -> ServiceResult<CreateInviteResultDto>;

    async fn accept_invite(&self, cmd: AcceptInviteCmd) -> ServiceResult<AcceptInviteResultDto>;

    async fn join_server_by_invite(
        &self,
        cmd: JoinServerByInviteCmd,
    ) -> ServiceResult<JoinServerByInviteResultDto>;

    /// Sign up using an operator-issued instance invite token.
    ///
    /// Atomically consumes one use slot and creates the user.
    /// On success the user has no workspace membership.
    ///
    /// # Errors
    /// - `InvalidToken` — token not found, exhausted, or revoked.
    /// - `ExpiredToken` — token exists but is past `expires_at`.
    /// - `Conflict` — username already taken.
    /// - `Validation` — username or password fails validation rules, or the invite is bound
    ///   to another email address.
    ///
    /// # Examples
    /// ```ignore
    /// let result = auth.signup_with_instance_invite(SignupWithInstanceInviteCmd {
    ///     instance_invite_token: raw_token,
    ///     username: "bob".into(),
    ///     password: "s3cur3p4ss".into(),
    /// }).await?;
    /// ```
    async fn signup_with_instance_invite(
        &self,
        cmd: SignupWithInstanceInviteCmd,
    ) -> ServiceResult<SignupResultDto>;

    /// Sign up in open-registration mode (no token required).
    ///
    /// Internally checks the signup rate limiter for `cmd.remote_ip` before
    /// creating the user. The calling resolver is responsible for verifying
    /// `RegistrationMode::Open` and supplying the remote IP.
    ///
    /// # Errors
    /// - `Conflict` — username already taken.
    /// - `Validation` — username or password fails validation rules.
    /// - `RateLimited` — signup rate limit exceeded for the caller's IP.
    ///
    /// # Examples
    /// ```ignore
    /// let result = auth.signup_open(SignupOpenCmd {
    ///     username: "carol".into(),
    ///     password: "s3cur3p4ss".into(),
    /// }).await?;
    /// ```
    async fn signup_open(&self, cmd: SignupOpenCmd) -> ServiceResult<SignupResultDto>;

    /// Preview a registration token without consuming it.
    ///
    /// Rate-limited at 30 req / 5 min / IP. Checks instance invites, then workspace invites.
    ///
    /// # Errors
    /// - `InvalidToken` — token not found, consumed, or revoked.
    /// - `ExpiredToken` — token exists but is past `expires_at`.
    /// - `RateLimited` — caller IP exceeded the preview rate limit.
    ///
    /// # Examples
    /// ```ignore
    /// let preview = auth.preview_invite_token(PreviewInviteTokenQuery {
    ///     token: raw_token,
    ///     remote_ip: "1.2.3.4".into(),
    /// }).await?;
    /// ```
    async fn preview_invite_token(
        &self,
        query: PreviewInviteTokenQuery,
    ) -> ServiceResult<InviteTokenPreviewDto>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RequestAuthenticator: Send + Sync {
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
    ) -> ServiceResult<RequestAuthResultDto>;
}
