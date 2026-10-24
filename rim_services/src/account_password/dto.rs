use chrono::{DateTime, Utc};
use rim_core::account_password::model::{PasswordLoginId, PasswordResetId};
use rim_core::auth::model::SessionId;

use crate::Actor;
use crate::key_auth::command::SignedPayload;

/// How many rewraps the server keeps per account.
pub const KEPT_REWRAPS: u32 = 3;

/// How long a login state, a password reset and a reset challenge stay valid, in seconds.
pub const PASSWORD_STEP_TTL_SECONDS: i64 = 120;

/// Longest OPAQUE message or rewrap the server reads, in bytes.
pub const MAX_PASSWORD_MESSAGE_LEN: usize = 1024;

/// Start registering a password for a principal that has no account here yet, before
/// `signupWithKey`.
#[derive(Debug, Clone)]
pub struct PasswordRegistrationStartCmd {
    pub principal_id: String,
    /// The OPAQUE `RegistrationRequest`.
    pub request: Vec<u8>,
    pub remote_ip: String,
}

/// Start a password login for a principal, before `loginWithKey`.
#[derive(Debug, Clone)]
pub struct PasswordLoginStartCmd {
    pub principal_id: String,
    /// The OPAQUE `CredentialRequest`.
    pub request: Vec<u8>,
    pub remote_ip: String,
}

/// The server's answer to a login start.
#[derive(Debug, Clone)]
pub struct PasswordLoginStartDto {
    /// Names the login state `loginWithKey` finishes.
    pub login_id: PasswordLoginId,
    /// The OPAQUE `CredentialResponse`.
    pub response: Vec<u8>,
}

/// Start changing the signed-in account's password: prove the current one and register the
/// new one.
#[derive(Debug, Clone)]
pub struct PasswordChangeStartCmd {
    pub actor: Actor,
    /// The OPAQUE `CredentialRequest` for the current password.
    pub credential_request: Vec<u8>,
    /// The OPAQUE `RegistrationRequest` for the new password.
    pub registration_request: Vec<u8>,
}

/// The server's answer to a change start.
#[derive(Debug, Clone)]
pub struct PasswordChangeStartDto {
    pub login_id: PasswordLoginId,
    pub credential_response: Vec<u8>,
    pub registration_response: Vec<u8>,
}

/// Finish a password change.
#[derive(Debug, Clone)]
pub struct PasswordChangeFinishCmd {
    pub actor: Actor,
    /// The session that changes the password; every other session of the account ends.
    pub session_id: SessionId,
    pub login_id: PasswordLoginId,
    /// The OPAQUE `CredentialFinalization` for the current password.
    pub credential_finalization: Vec<u8>,
    /// The OPAQUE `RegistrationUpload` for the new password.
    pub registration_upload: Vec<u8>,
    /// The previous unlock key sealed under the new one, for the account's other devices.
    pub rewrap: Vec<u8>,
}

/// Ask for a challenge to sign a password reset over.
#[derive(Debug, Clone)]
pub struct PasswordResetChallengeCmd {
    pub principal_id: String,
    pub remote_ip: String,
}

/// Start a password reset proved by the recovery key.
#[derive(Debug, Clone)]
pub struct PasswordResetStartCmd {
    /// A `password-reset` statement signed by the recovery key of the key log head.
    pub statement: SignedPayload,
    /// The OPAQUE `RegistrationRequest` for the new password.
    pub registration_request: Vec<u8>,
    pub remote_ip: String,
}

/// The server's answer to a reset start.
#[derive(Debug, Clone)]
pub struct PasswordResetStartDto {
    /// Names the reset `passwordResetFinish` completes.
    pub reset_id: PasswordResetId,
    pub registration_response: Vec<u8>,
}

/// Finish a password reset.
#[derive(Debug, Clone)]
pub struct PasswordResetFinishCmd {
    pub reset_id: PasswordResetId,
    /// The OPAQUE `RegistrationUpload` for the new password.
    pub registration_upload: Vec<u8>,
}

/// Start setting a password for a signed-in account that has none.
#[derive(Debug, Clone)]
pub struct SetPasswordStartCmd {
    pub actor: Actor,
    /// The OPAQUE `RegistrationRequest`.
    pub request: Vec<u8>,
}

/// Finish setting a password for a signed-in account that has none.
#[derive(Debug, Clone)]
pub struct SetPasswordCmd {
    pub actor: Actor,
    /// The OPAQUE `RegistrationUpload`.
    pub upload: Vec<u8>,
}

/// What setting a password returns: the account's new server share, in the clear, for the
/// device to derive its unlock key.
#[derive(Debug, Clone)]
pub struct SetPasswordDto {
    pub server_share: Vec<u8>,
}

/// When a password change took effect.
#[derive(Debug, Clone)]
pub struct PasswordChangedDto {
    pub changed_at: DateTime<Utc>,
}
