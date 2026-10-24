use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::account_password::dto::{
    PasswordChangeFinishCmd, PasswordChangeStartCmd, PasswordChangeStartDto, PasswordChangedDto,
    PasswordLoginFinishCmd, PasswordLoginFinishDto, PasswordLoginStartCmd, PasswordLoginStartDto,
    PasswordRegistrationStartCmd, PasswordResetChallengeCmd, PasswordResetFinishCmd,
    PasswordResetStartCmd, PasswordResetStartDto, SetPasswordCmd, SetPasswordDto,
    SetPasswordStartCmd,
};
use crate::key_auth::dto::KeyChallengeDto;

/// The password of local accounts, checked with OPAQUE: registering it before a signup,
/// running a login whose ticket `loginWithKey` consumes, changing it, resetting it with the recovery
/// key, and setting it for an account that has none.
///
/// Every login start counts as a wrong try until a login finishes: tries 1 to 3 are free, the
/// 4th to the 9th make the next one wait longer, and the 10th locks the password until a reset.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AccountPasswordService: Send + Sync {
    /// Answer the first message of a password registration for a principal that has no local
    /// account here. Returns the OPAQUE `RegistrationResponse`; the upload goes with
    /// `signupWithKey`.
    ///
    /// # Errors
    /// - `Conflict` when a local account here has the principal.
    /// - `Validation` for a malformed principal id or request.
    /// - `RateLimited` when the caller's IP asks too often.
    async fn registration_start(&self, cmd: PasswordRegistrationStartCmd)
    -> ServiceResult<Vec<u8>>;

    /// Count a login try against the principal's password and answer it. A principal without a
    /// local password gets an answer of the same shape that no password finishes, and nothing
    /// is counted.
    ///
    /// # Errors
    /// - `PasswordLocked` after ten tries without a success.
    /// - `RateLimited` while the last try's wait runs, or when the caller's IP fails too often.
    /// - `Validation` for a malformed principal id or request.
    async fn login_start(&self, cmd: PasswordLoginStartCmd)
    -> ServiceResult<PasswordLoginStartDto>;

    /// Check the client's finalization against a login started with [`Self::login_start`].
    /// On success clear the tries and answer a single-use ticket for `loginWithKey`, valid for
    /// 120 seconds, the server share sealed under the login's transport key, and the rewraps.
    /// The login state is used once, whatever the outcome.
    ///
    /// # Errors
    /// `Unauthenticated` when the login is unknown, expired or not a sign-in, or the password is
    /// wrong.
    async fn login_finish(
        &self,
        cmd: PasswordLoginFinishCmd,
    ) -> ServiceResult<PasswordLoginFinishDto>;

    /// Prove the current password and answer the new one's registration. Counts as a try.
    ///
    /// # Errors
    /// - `NotFound` when the account has no password.
    /// - `PasswordLocked` and `RateLimited` as for [`Self::login_start`].
    /// - `Validation` for a malformed request.
    async fn change_start(
        &self,
        cmd: PasswordChangeStartCmd,
    ) -> ServiceResult<PasswordChangeStartDto>;

    /// Replace the password once the current one verified, keep the server share, store the
    /// rewrap (the newest three are kept) and end every other session of the account.
    ///
    /// # Errors
    /// - `Unauthenticated` when the login state is unknown, expired, another account's or not
    ///   a change, or the current password does not verify.
    /// - `Validation` for a malformed upload or rewrap.
    async fn change_finish(
        &self,
        cmd: PasswordChangeFinishCmd,
    ) -> ServiceResult<PasswordChangedDto>;

    /// Issue a single-use challenge, valid for 120 seconds, for a `password-reset` statement.
    ///
    /// # Errors
    /// `RateLimited` when the caller's IP asks too often; `Validation` for a malformed
    /// principal id.
    async fn reset_challenge(
        &self,
        cmd: PasswordResetChallengeCmd,
    ) -> ServiceResult<KeyChallengeDto>;

    /// Verify a `password-reset` statement signed by the recovery key of the account's key log
    /// head, claim its challenge, and answer the new password's registration.
    ///
    /// # Errors
    /// - `Unauthenticated` for an unknown principal or an account of another server, a
    ///   signature by any other key, a wrong audience, or a challenge that is unknown, used or
    ///   expired. Deliberately not told apart.
    /// - `Validation` for a malformed statement or request.
    /// - `RateLimited` when the caller's IP fails too often.
    async fn reset_start(&self, cmd: PasswordResetStartCmd)
    -> ServiceResult<PasswordResetStartDto>;

    /// Replace the account's password, keeping its server share (or making one for an account
    /// without a password), delete its rewraps, clear its tries and lock, and end every session
    /// of the account.
    ///
    /// # Errors
    /// `Unauthenticated` when the reset is unknown, used or expired; `Validation` for a
    /// malformed upload.
    async fn reset_finish(&self, cmd: PasswordResetFinishCmd) -> ServiceResult<()>;

    /// Answer the first message of a password registration for the signed-in account, which
    /// has none.
    ///
    /// # Errors
    /// `Conflict` when the account has a password; `Validation` for an account of another
    /// server or without a key identity, or a malformed request.
    async fn set_password_start(&self, cmd: SetPasswordStartCmd) -> ServiceResult<Vec<u8>>;

    /// Store the signed-in account's first password with a new server share, and lift the
    /// limit from its sessions.
    ///
    /// # Errors
    /// As for [`Self::set_password_start`].
    async fn set_password(&self, cmd: SetPasswordCmd) -> ServiceResult<SetPasswordDto>;
}
