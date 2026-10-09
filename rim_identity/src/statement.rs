use ed25519_dalek::{Signature, Signer, SigningKey};
use serde::de::DeserializeOwned;

use crate::encoding::{PublicKey, b64, b64_fixed};
use crate::error::{IdentityError, IdentityResult};

/// The statement format version this crate reads and writes.
pub const STATEMENT_VERSION: u32 = 1;

/// A JSON statement and its Ed25519 signature, both as sent on the wire.
///
/// `payload` is the exact UTF-8 text that was signed. Verifiers check the signature over these
/// bytes and only then parse them; nothing is ever re-serialized, so there is no canonical JSON
/// form to get wrong. Servers store the payload as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedStatement {
    pub payload: String,
    /// Unpadded base64url of the 64-byte signature.
    pub signature: String,
}

/// What a signature is for. Each purpose signs under its own context string, so a signature
/// made for one kind of statement never verifies as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    KeyLog,
    Device,
    DeviceRevoke,
    ProfileCard,
    Login,
}

impl Purpose {
    /// Bytes signed in front of the payload.
    pub fn context(self) -> &'static [u8] {
        match self {
            Self::KeyLog => b"rim.keylog.v1\0",
            Self::Device => b"rim.device.v1\0",
            Self::DeviceRevoke => b"rim.device-revoke.v1\0",
            Self::ProfileCard => b"rim.profile-card.v1\0",
            Self::Login => b"rim.login.v1\0",
        }
    }

    /// Largest payload accepted, in bytes.
    pub fn max_payload_len(self) -> usize {
        match self {
            Self::ProfileCard => 16 * 1024,
            _ => 4 * 1024,
        }
    }
}

fn signed_message(purpose: Purpose, payload: &str) -> Vec<u8> {
    let context = purpose.context();
    let mut message = Vec::with_capacity(context.len() + payload.len());
    message.extend_from_slice(context);
    message.extend_from_slice(payload.as_bytes());
    message
}

/// Sign `payload` for `purpose`. Clients and tests use this; the server only verifies.
pub fn sign(purpose: Purpose, key: &SigningKey, payload: impl Into<String>) -> SignedStatement {
    let payload = payload.into();
    let signature = key.sign(&signed_message(purpose, &payload));
    SignedStatement {
        payload,
        signature: b64(&signature.to_bytes()),
    }
}

/// Check `statement`'s signature by `key`, with the strict checks that reject malleable
/// signatures and small-order keys.
pub(crate) fn verify(
    purpose: Purpose,
    key: &PublicKey,
    statement: &SignedStatement,
) -> IdentityResult<()> {
    let bytes = b64_fixed::<64>(&statement.signature, "signature")?;
    let signature = Signature::from_bytes(&bytes);
    key.verifying_key()
        .verify_strict(&signed_message(purpose, &statement.payload), &signature)
        .map_err(|_| IdentityError::BadSignature)
}

/// Parse a payload after a size check. Structs passed here deny unknown fields, and serde
/// refuses duplicate fields, so each payload has one meaning.
pub(crate) fn parse<T: DeserializeOwned>(purpose: Purpose, payload: &str) -> IdentityResult<T> {
    let max = purpose.max_payload_len();
    if payload.len() > max {
        return Err(IdentityError::TooLarge { max });
    }
    serde_json::from_str(payload).map_err(|e| IdentityError::Malformed(e.to_string()))
}

/// Check the `v` and `type` fields every statement starts with.
pub(crate) fn check_header(v: u32, kind: &str, expected: &'static str) -> IdentityResult<()> {
    if v != STATEMENT_VERSION {
        return Err(IdentityError::UnsupportedVersion(v));
    }
    if kind != expected {
        return Err(IdentityError::WrongType { expected });
    }
    Ok(())
}

/// Validate a short human-readable label: trimmed, non-empty, at most `max` characters, no
/// control characters. Returns the trimmed text.
pub(crate) fn check_label(value: &str, field: &str, max: usize) -> IdentityResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max {
        return Err(IdentityError::Invalid(format!(
            "{field} must be 1 to {max} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(IdentityError::Invalid(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[3; 32])
    }

    #[test]
    fn signature_verifies_only_for_its_purpose() {
        let stmt = sign(Purpose::Device, &key(), "{}");
        let public = PublicKey::from(&key());
        assert_eq!(verify(Purpose::Device, &public, &stmt), Ok(()));
        assert_eq!(
            verify(Purpose::Login, &public, &stmt),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn tampered_payload_fails() {
        let mut stmt = sign(Purpose::Login, &key(), r#"{"a":1}"#);
        stmt.payload = r#"{"a":2}"#.to_string();
        assert_eq!(
            verify(Purpose::Login, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn signature_with_padding_or_wrong_length_is_bad_encoding() {
        let mut stmt = sign(Purpose::Login, &key(), "{}");
        stmt.signature.push('=');
        assert_eq!(
            verify(Purpose::Login, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadEncoding("signature"))
        );
        stmt.signature = b64(&[0; 63]);
        assert_eq!(
            verify(Purpose::Login, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadEncoding("signature"))
        );
    }

    #[test]
    fn parse_rejects_oversized_payload() {
        let big = format!("\"{}\"", "a".repeat(4 * 1024));
        assert_eq!(
            parse::<String>(Purpose::Login, &big),
            Err(IdentityError::TooLarge { max: 4096 })
        );
    }

    #[test]
    fn check_label_trims_and_bounds() {
        assert_eq!(check_label("  Laptop ", "name", 64), Ok("Laptop".into()));
        assert!(check_label("   ", "name", 64).is_err());
        assert!(check_label(&"x".repeat(65), "name", 64).is_err());
        assert!(check_label("a\u{7}b", "name", 64).is_err());
    }
}
