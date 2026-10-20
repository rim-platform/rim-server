use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::repository::pagination::Page;

use crate::ServiceResult;
use crate::auth::command::{
    AcceptInviteCmd, CreateInviteCmd, JoinServerByInviteCmd, ListInvitesQuery, LoginCmd, LogoutCmd,
    LogoutOtherSessionsCmd, PreviewInviteTokenQuery, RevokeInviteCmd, SignupOpenCmd,
    SignupWithInstanceInviteCmd,
};
use crate::auth::dto::{
    AcceptInviteResultDto, CreateInviteResultDto, InviteDto, InvitePageCursor,
    InviteTokenPreviewDto, JoinServerByInviteResultDto, LoginResultDto, RequestAuthResultDto,
    SignupResultDto,
};

/// Password sign-in, sessions, invites and the password signups. A server whose signups need a
/// key refuses every password signup with `Forbidden`; sign-in of existing accounts is
/// unaffected.
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

    /// Create a workspace invite. Owner only. `expires_at` defaults to seven days from now and
    /// may be at most thirty days out.
    ///
    /// # Errors
    /// - `PermissionDenied` — the actor does not own the workspace.
    /// - `Validation` — the expiry is not in the future or too far out, the uses or the role
    ///   are out of range, or the recipient email is malformed.
    async fn create_invite(&self, cmd: CreateInviteCmd) -> ServiceResult<CreateInviteResultDto>;

    /// Revoke a workspace invite, so it admits nobody from now on; members who joined with it
    /// stay. Owner of the invite's workspace only. Returns the invite; revoking a revoked
    /// invite returns it unchanged.
    ///
    /// # Errors
    /// - `NotFound` — no such invite, or the actor is not a member of its workspace.
    /// - `PermissionDenied` — the actor is a member of the invite's workspace but not its owner.
    async fn revoke_invite(&self, cmd: RevokeInviteCmd) -> ServiceResult<InviteDto>;

    /// The invites of a workspace, newest first, whatever their state. Owner only.
    ///
    /// # Errors
    /// `PermissionDenied` — the actor does not own the workspace.
    async fn list_invites(
        &self,
        query: ListInvitesQuery,
    ) -> ServiceResult<Page<InviteDto, InvitePageCursor>>;

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

    /// Whether the instance holds as many local accounts as its cap allows, so every signup
    /// answers `RegistrationFull`; `false` when there is no cap. The answer may be up to half a
    /// minute old; a signup always counts afresh.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn registration_full(&self) -> ServiceResult<bool>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RequestAuthenticator: Send + Sync {
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
    ) -> ServiceResult<RequestAuthResultDto>;

    /// The sessions among `ids` that are still signed in: neither revoked nor expired.
    async fn live_sessions(
        &self,
        ids: &[rim_core::auth::model::SessionId],
    ) -> ServiceResult<Vec<rim_core::auth::model::SessionId>>;
}
