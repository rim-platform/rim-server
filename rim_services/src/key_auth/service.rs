use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::auth::dto::LoginResultDto;
use crate::key_auth::command::{
    AddDeviceKeyCmd, EnrollIdentityKeyCmd, GetKeyLogQuery, GetMyKeyIdentityQuery,
    IssueKeyChallengeCmd, LoginWithKeyCmd, PublishProfileCardCmd, RevokeDeviceKeyCmd,
    RotateIdentityKeyCmd,
};
use crate::key_auth::dto::{
    DeviceKeyDto, KeyChallengeDto, KeyIdentityDto, KeyLogEntryDto, ProfileCardDto,
};

/// Key identity and challenge login (Stage 3, `docs/specs/key-identity-v1.md`).
///
/// Sits beside the password [`crate::auth::service::AuthService`]: a key login issues the same
/// session the password login does. Once an account enrolls a key, its password no longer signs
/// in.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait KeyAuthService: Send + Sync {
    /// Issue a single-use challenge for a device to sign. No session needed.
    ///
    /// # Errors
    /// `RateLimited` when the caller's IP asks too often.
    async fn issue_challenge(&self, cmd: IssueKeyChallengeCmd) -> ServiceResult<KeyChallengeDto>;

    /// Sign in with a `login` statement over a challenge, signed by an enrolled device key.
    ///
    /// # Errors
    /// - `Unauthenticated` for an unknown principal or device, a bad signature, a wrong audience,
    ///   or a challenge that is unknown, used or expired. Deliberately not told apart.
    /// - `PermissionDenied` when the signature is valid but the device was revoked.
    /// - `RateLimited` when the caller's IP fails too often.
    async fn login_with_key(&self, cmd: LoginWithKeyCmd) -> ServiceResult<LoginResultDto>;

    /// Bind a key identity to the signed-in account, with its first device. The calling session
    /// becomes that device's session, and the password stops signing in.
    ///
    /// # Errors
    /// - `Validation` when the log or the certificate does not verify.
    /// - `Conflict` when the account already has a key, or another account here has this one.
    async fn enroll(&self, cmd: EnrollIdentityKeyCmd) -> ServiceResult<KeyIdentityDto>;

    /// Add a device certified by the current identity key.
    ///
    /// # Errors
    /// - `Validation` when the certificate does not verify against the current key log, or the
    ///   account has no key.
    /// - `Conflict` when the device id or key is already enrolled (or was revoked).
    async fn add_device(&self, cmd: AddDeviceKeyCmd) -> ServiceResult<DeviceKeyDto>;

    /// Revoke a device, signed by the identity key or by the device itself. Ends its sessions.
    /// Revoking a revoked device is a no-op.
    ///
    /// # Errors
    /// `NotFound` for an unknown device, `Validation` when the statement does not verify.
    async fn revoke_device(&self, cmd: RevokeDeviceKeyCmd) -> ServiceResult<DeviceKeyDto>;

    /// Append a rotation to the key log. A recovery rotation revokes every device and session of
    /// the account and enrolls the device given with it.
    ///
    /// # Errors
    /// - `Unauthenticated` for an identity-signed rotation without a session.
    /// - `PermissionDenied` when the session belongs to another account.
    /// - `NotFound` when no account here has the principal.
    /// - `Validation` when the entry or device certificate does not verify, or a recovery
    ///   rotation comes without a device.
    async fn rotate(&self, cmd: RotateIdentityKeyCmd) -> ServiceResult<KeyIdentityDto>;

    /// Store a profile card signed by the current identity key.
    ///
    /// # Errors
    /// `Validation` when it does not verify, `Conflict` when its version is not greater than the
    /// stored card's.
    async fn publish_profile_card(
        &self,
        cmd: PublishProfileCardCmd,
    ) -> ServiceResult<ProfileCardDto>;

    /// The caller's key log, devices and card.
    async fn get_my_key_identity(
        &self,
        query: GetMyKeyIdentityQuery,
    ) -> ServiceResult<KeyIdentityDto>;

    /// The key log of the principal with this id, oldest first; empty when no account here has
    /// it. Works without a session: the log is public (every server the person uses holds it),
    /// and a person who lost every device needs its head to sign a recovery rotation.
    async fn get_key_log(&self, query: GetKeyLogQuery) -> ServiceResult<Vec<KeyLogEntryDto>>;
}
