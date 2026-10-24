//! The account password roots' answers. Bytes travel as unpadded base64url strings.

use async_graphql::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rim_services::ServiceError;
use rim_services::account_password::dto::{
    PasswordChangeStartDto, PasswordChangedDto, PasswordLoginFinishDto, PasswordLoginStartDto,
    PasswordResetStartDto,
};

use crate::errors::map_service_error;

/// `value` as unpadded base64url.
pub(crate) fn encode_bytes(value: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(value)
}

/// The bytes the unpadded base64url argument `field` holds.
///
/// # Errors
/// `VALIDATION` when it is not unpadded base64url.
pub(crate) fn decode_bytes(value: &str, field: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value.trim()).map_err(|_| {
        map_service_error(ServiceError::Validation(format!(
            "{field} is not unpadded base64url"
        )))
    })
}

/// The server's answer to `passwordLoginStart`.
#[derive(SimpleObject, Clone, Debug)]
pub struct PasswordLoginStart {
    /// Pass to `passwordLoginFinish` with the credential finalization. Valid for 120 seconds.
    pub login_id: ID,
    /// The OPAQUE `CredentialResponse`.
    pub response: String,
}

impl From<PasswordLoginStartDto> for PasswordLoginStart {
    fn from(value: PasswordLoginStartDto) -> Self {
        Self {
            login_id: ID(value.login_id.to_string()),
            response: encode_bytes(&value.response),
        }
    }
}

/// A finished password login.
#[derive(SimpleObject, Clone, Debug)]
pub struct PasswordLoginFinish {
    /// Pass to `loginWithKey` as `passwordTicket`: single use, valid for 120 seconds, for this
    /// account only.
    pub ticket: String,
    /// The account's server share, AES-256-GCM sealed under the login's transport key for
    /// "server share": unpadded base64url of the 12-byte nonce, the ciphertext and the tag.
    pub server_share: String,
    /// The account's rewraps, newest first.
    pub rewraps: Vec<String>,
}

impl From<PasswordLoginFinishDto> for PasswordLoginFinish {
    fn from(value: PasswordLoginFinishDto) -> Self {
        Self {
            ticket: value.ticket,
            server_share: value.server_share,
            rewraps: value.rewraps.iter().map(|r| encode_bytes(r)).collect(),
        }
    }
}

/// The server's answer to `passwordChangeStart`.
#[derive(SimpleObject, Clone, Debug)]
pub struct PasswordChangeStart {
    /// Pass to `passwordChangeFinish`. Valid for 120 seconds.
    pub login_id: ID,
    /// The OPAQUE `CredentialResponse` for the current password.
    pub credential_response: String,
    /// The OPAQUE `RegistrationResponse` for the new password.
    pub registration_response: String,
}

impl From<PasswordChangeStartDto> for PasswordChangeStart {
    fn from(value: PasswordChangeStartDto) -> Self {
        Self {
            login_id: ID(value.login_id.to_string()),
            credential_response: encode_bytes(&value.credential_response),
            registration_response: encode_bytes(&value.registration_response),
        }
    }
}

/// A finished password change.
#[derive(SimpleObject, Clone, Debug)]
pub struct PasswordChanged {
    pub changed_at: DateTime<Utc>,
}

impl From<PasswordChangedDto> for PasswordChanged {
    fn from(value: PasswordChangedDto) -> Self {
        Self {
            changed_at: value.changed_at,
        }
    }
}

/// The server's answer to `passwordResetStart`.
#[derive(SimpleObject, Clone, Debug)]
pub struct PasswordResetStart {
    /// Pass to `passwordResetFinish`. Valid for 120 seconds, once.
    pub reset_id: ID,
    /// The OPAQUE `RegistrationResponse` for the new password.
    pub registration_response: String,
}

impl From<PasswordResetStartDto> for PasswordResetStart {
    fn from(value: PasswordResetStartDto) -> Self {
        Self {
            reset_id: ID(value.reset_id.to_string()),
            registration_response: encode_bytes(&value.registration_response),
        }
    }
}
