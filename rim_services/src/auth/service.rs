use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::repository::pagination::Page;

use crate::ServiceResult;
use crate::auth::command::{
    CreateInviteCmd, JoinServerByInviteCmd, ListInvitesQuery, LogoutCmd, LogoutOtherSessionsCmd,
    PreviewInviteTokenQuery, RevokeInviteCmd,
};
use crate::auth::dto::{
    CreateInviteResultDto, InviteDto, InvitePageCursor, InviteTokenPreviewDto,
    JoinServerByInviteResultDto, RequestAuthResultDto, SessionSecret,
};

/// Sessions, workspace invites, the invite token preview and the account cap.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AuthService: Send + Sync {
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

    async fn join_server_by_invite(
        &self,
        cmd: JoinServerByInviteCmd,
    ) -> ServiceResult<JoinServerByInviteResultDto>;

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

    /// Count local accounts before validating signup inputs, without reserving a place.
    ///
    /// # Errors
    /// `RegistrationFull` at the account cap; storage errors propagate.
    async fn check_registration_capacity(&self) -> ServiceResult<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RequestAuthenticator: Send + Sync {
    /// The user, session and device of the live session whose cookie is `session_token`.
    /// Anonymous without a token, for an unknown, revoked or expired session, and for an
    /// unkept session when `secret` is required and does not match its secret.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn authenticate_session_token(
        &self,
        session_token: Option<&str>,
        secret: SessionSecret,
    ) -> ServiceResult<RequestAuthResultDto>;

    /// The sessions among `ids` that are still signed in: neither revoked nor expired.
    async fn live_sessions(
        &self,
        ids: &[rim_core::auth::model::SessionId],
    ) -> ServiceResult<Vec<rim_core::auth::model::SessionId>>;
}
