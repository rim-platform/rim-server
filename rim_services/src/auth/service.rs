use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInstanceInviteCmd, CreateInviteCmd, JoinServerByInviteCmd,
    ListInvitesQuery, LoginCmd, LogoutCmd, PreviewInviteTokenQuery, RevokeInstanceInviteCmd,
    RevokeInviteCmd, SignupOpenCmd, SignupWithBootstrapCmd, SignupWithInstanceInviteCmd,
};
use crate::auth::dto::{
    AcceptInviteResultDto, BootstrapResultDto, CreateInstanceInviteResultDto,
    CreateInviteResultDto, InstanceInviteDto, InviteDto, InviteTokenPreviewDto,
    JoinServerByInviteResultDto, LoginResultDto, RequestAuthResultDto, SignupResultDto,
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

    async fn list_invites(&self, query: ListInvitesQuery) -> ServiceResult<Vec<InviteDto>>;

    async fn revoke_invite(&self, cmd: RevokeInviteCmd) -> ServiceResult<()>;

    /// Sign up using a CLI-issued bootstrap token.
    ///
    /// Consumes the token in the same transaction as user creation.
    /// On success the user has no workspace membership.
    ///
    /// # Errors
    /// - `InvalidToken` — token not found, already used, or revoked.
    /// - `ExpiredToken` — token exists but is past `expires_at`.
    /// - `Conflict` — username already taken.
    /// - `Validation` — username or password fails validation rules.
    ///
    /// # Examples
    /// ```ignore
    /// let result = auth.signup_with_bootstrap(SignupWithBootstrapCmd {
    ///     bootstrap_token: raw_token,
    ///     username: "alice".into(),
    ///     password: "s3cur3p4ss".into(),
    /// }).await?;
    /// ```
    async fn signup_with_bootstrap(
        &self,
        cmd: SignupWithBootstrapCmd,
    ) -> ServiceResult<SignupResultDto>;

    /// Sign up using an admin-issued instance invite token.
    ///
    /// Atomically consumes one use slot and creates the user.
    /// On success the user has no workspace membership.
    ///
    /// # Errors
    /// - `InvalidToken` — token not found, exhausted, or revoked.
    /// - `ExpiredToken` — token exists but is past `expires_at`.
    /// - `Conflict` — username already taken.
    /// - `Validation` — username or password fails validation rules.
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

    /// Issue a new instance invite.
    ///
    /// Requires the caller (`actor`) to be the `Owner` of at least one server
    /// on this instance (placeholder gate — see spec §11.1).
    ///
    /// # Errors
    /// - `PermissionDenied` — actor is not a workspace owner on any server.
    /// - `Validation` — `expires_at` is in the past, or `max_uses` exceeds 50.
    ///
    /// # Examples
    /// ```ignore
    /// let result = auth.create_instance_invite(CreateInstanceInviteCmd {
    ///     actor,
    ///     expires_at: Some(Utc::now() + Duration::days(7)),
    ///     max_uses: Some(5),
    /// }).await?;
    /// ```
    async fn create_instance_invite(
        &self,
        cmd: CreateInstanceInviteCmd,
    ) -> ServiceResult<CreateInstanceInviteResultDto>;

    /// Revoke an existing instance invite.
    ///
    /// Requires the caller to be a workspace owner.
    ///
    /// # Errors
    /// - `NotFound` — invite id does not exist.
    /// - `PermissionDenied` — actor is not a workspace owner on any server.
    ///
    /// # Examples
    /// ```ignore
    /// auth.revoke_instance_invite(RevokeInstanceInviteCmd {
    ///     actor,
    ///     invite_id,
    /// }).await?;
    /// ```
    async fn revoke_instance_invite(&self, cmd: RevokeInstanceInviteCmd) -> ServiceResult<()>;

    /// List all active (non-revoked) instance invites.
    ///
    /// Requires the caller to be a workspace owner (same gate as issuance).
    ///
    /// # Errors
    /// - `PermissionDenied` — actor is not a workspace owner.
    ///
    /// # Examples
    /// ```ignore
    /// let invites = auth.list_instance_invites(actor).await?;
    /// ```
    async fn list_instance_invites(
        &self,
        actor: crate::Actor,
    ) -> ServiceResult<Vec<InstanceInviteDto>>;

    /// Preview a registration token without consuming it.
    ///
    /// Rate-limited at 30 req / 5 min / IP. Checks bootstrap tokens,
    /// instance invites, and workspace invites in that order.
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
