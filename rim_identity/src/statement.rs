use ed25519_dalek::{Signer, SigningKey};
use serde::de::DeserializeOwned;

use crate::encoding::{PublicKey, Signature};
use crate::error::{IdentityError, IdentityResult};

/// The statement format versions this crate reads (the payload's `"v"` field).
///
/// The two versions differ only in how keys and signatures are spelled: version 1 writes an
/// Ed25519 key or signature as bare unpadded base64url, version 2 tags every key and signature
/// with its algorithm (`ed25519:<base64url>`), so later algorithms fit without a new format.
/// Field names, contexts and every rule are the same. Clients write version 2; version 1 stays
/// accepted so statements signed before version 2 existed keep verifying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatementVersion {
    V1,
    V2,
}

impl StatementVersion {
    /// The value of the `"v"` field.
    pub fn number(self) -> u32 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }

    pub(crate) fn from_number(v: u32) -> IdentityResult<Self> {
        match v {
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            _ => Err(IdentityError::UnsupportedVersion(v)),
        }
    }
}

/// A JSON statement and its signature, both as sent on the wire.
///
/// `payload` is the exact UTF-8 text that was signed. Verifiers check the signature over these
/// bytes and only then parse them; nothing is ever re-serialized, so there is no canonical JSON
/// form to get wrong. Servers store the payload as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedStatement {
    pub payload: String,
    /// The signature, spelled as the payload's version requires: bare unpadded base64url of the
    /// 64-byte Ed25519 signature in version 1, `<algorithm>:<base64url>` in version 2.
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
    PluginManifest,
    MlsKey,
    MlsReadd,
    DeviceRequest,
    Delegation,
    PasswordReset,
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
            Self::PluginManifest => b"rim.plugin-manifest.v1\0",
            Self::MlsKey => b"rim.mls-key.v1\0",
            Self::MlsReadd => b"rim.mls-readd.v1\0",
            Self::DeviceRequest => b"rim.device-request.v1\0",
            Self::Delegation => b"rim.delegation.v1\0",
            Self::PasswordReset => b"rim.password-reset.v1\0",
        }
    }

    /// Largest payload accepted, in bytes.
    ///
    /// Sized for hybrid Ed25519 + ML-DSA keys, whose tagged text is about 1.8 KB for ML-DSA-44
    /// and 3.5 KB for ML-DSA-87: a key log rotation names two keys, and a profile card lists up
    /// to 32 device keys. A plugin manifest embeds a manifest of up to 64 KiB. Version 1
    /// statements share the caps.
    pub fn max_payload_len(self) -> usize {
        match self {
            Self::ProfileCard => 128 * 1024,
            Self::PluginManifest => 96 * 1024,
            _ => 16 * 1024,
        }
    }
}

fn signed_message(purpose: Purpose, payload: &str) -> Vec<u8> {
    context_message(purpose.context(), payload)
}

/// Sign `payload` for `purpose` with an Ed25519 key, spelling the signature as `version`
/// requires; `version` must match the payload's `"v"` field. Clients and tests use this; the
/// server only verifies.
pub fn sign(
    purpose: Purpose,
    version: StatementVersion,
    key: &SigningKey,
    payload: impl Into<String>,
) -> SignedStatement {
    let payload = payload.into();
    let signature = Signature::Ed25519(key.sign(&signed_message(purpose, &payload)));
    SignedStatement {
        payload,
        signature: signature.encode_for(version),
    }
}

/// Sign `payload` as a version 2 statement under `context`, for statement kinds defined outside
/// this crate (server-signed statements). The signature is tagged with the key's algorithm.
pub fn sign_with_context(
    context: &[u8],
    key: &SigningKey,
    payload: impl Into<String>,
) -> SignedStatement {
    let payload = payload.into();
    let signature = Signature::Ed25519(key.sign(&context_message(context, &payload)));
    SignedStatement {
        payload,
        signature: signature.encode_for(StatementVersion::V2),
    }
}

/// Check a version 2 statement's signature over `context || payload` by `key`. The signature's
/// algorithm tag must be the key's.
pub fn verify_with_context(
    context: &[u8],
    key: &PublicKey,
    statement: &SignedStatement,
) -> IdentityResult<()> {
    let signature = Signature::decode_for(StatementVersion::V2, &statement.signature, key)?;
    key.verify(&context_message(context, &statement.payload), &signature)
}

fn context_message(context: &[u8], payload: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(context.len() + payload.len());
    message.extend_from_slice(context);
    message.extend_from_slice(payload.as_bytes());
    message
}

/// Check `statement`'s signature by `key`. `version` is the payload's, read by
/// [`check_header`]; it decides how the signature is spelled.
pub(crate) fn verify(
    purpose: Purpose,
    version: StatementVersion,
    key: &PublicKey,
    statement: &SignedStatement,
) -> IdentityResult<()> {
    let signature = Signature::decode_for(version, &statement.signature, key)?;
    key.verify(&signed_message(purpose, &statement.payload), &signature)
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

/// Check the `v` and `type` fields every statement starts with, and return the version.
pub(crate) fn check_header(
    v: u32,
    kind: &str,
    expected: &'static str,
) -> IdentityResult<StatementVersion> {
    let version = StatementVersion::from_number(v)?;
    if kind != expected {
        return Err(IdentityError::WrongType { expected });
    }
    Ok(version)
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
    use crate::encoding::b64;

    const V1: StatementVersion = StatementVersion::V1;
    const V2: StatementVersion = StatementVersion::V2;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[3; 32])
    }

    #[test]
    fn signature_verifies_only_for_its_purpose() {
        for version in [V1, V2] {
            let stmt = sign(Purpose::Device, version, &key(), "{}");
            let public = PublicKey::from(&key());
            assert_eq!(verify(Purpose::Device, version, &public, &stmt), Ok(()));
            assert_eq!(
                verify(Purpose::Login, version, &public, &stmt),
                Err(IdentityError::BadSignature)
            );
        }
    }

    #[test]
    fn context_signature_is_tagged_and_bound_to_its_context() {
        let stmt = sign_with_context(b"rim.test.v1\0", &key(), "{}");
        assert!(stmt.signature.starts_with("ed25519:"));
        let public = PublicKey::from(&key());
        assert_eq!(
            verify_with_context(b"rim.test.v1\0", &public, &stmt),
            Ok(())
        );
        assert_eq!(
            verify_with_context(b"rim.other.v1\0", &public, &stmt),
            Err(IdentityError::BadSignature)
        );
        assert_eq!(
            verify_with_context(
                Purpose::Login.context(),
                &public,
                &sign(Purpose::Login, V2, &key(), "{}")
            ),
            Ok(())
        );
    }

    #[test]
    fn tampered_payload_fails() {
        let mut stmt = sign(Purpose::Login, V1, &key(), r#"{"a":1}"#);
        stmt.payload = r#"{"a":2}"#.to_string();
        assert_eq!(
            verify(Purpose::Login, V1, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn signature_with_padding_or_wrong_length_is_bad_encoding() {
        let mut stmt = sign(Purpose::Login, V1, &key(), "{}");
        stmt.signature.push('=');
        assert_eq!(
            verify(Purpose::Login, V1, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadEncoding("signature"))
        );
        stmt.signature = b64(&[0; 63]);
        assert_eq!(
            verify(Purpose::Login, V1, &PublicKey::from(&key()), &stmt),
            Err(IdentityError::BadEncoding("signature"))
        );
    }

    #[test]
    fn version_2_signature_is_tagged_and_version_1_is_bare() {
        let v1 = sign(Purpose::Login, V1, &key(), "{}");
        let v2 = sign(Purpose::Login, V2, &key(), "{}");
        assert_eq!(v2.signature, format!("ed25519:{}", v1.signature));
        let public = PublicKey::from(&key());

        // Each version accepts only its own spelling.
        assert_eq!(
            verify(Purpose::Login, V1, &public, &v2),
            Err(IdentityError::BadEncoding("signature"))
        );
        assert_eq!(
            verify(Purpose::Login, V2, &public, &v1),
            Err(IdentityError::BadEncoding("signature"))
        );
    }

    #[test]
    fn version_2_signature_with_unknown_or_malformed_tag_is_refused() {
        let v2 = sign(Purpose::Login, V2, &key(), "{}");
        let body = v2.signature.strip_prefix("ed25519:").expect("tagged");
        let public = PublicKey::from(&key());
        let with_tag = |tag: &str| SignedStatement {
            payload: v2.payload.clone(),
            signature: format!("{tag}:{body}"),
        };
        assert_eq!(
            verify(Purpose::Login, V2, &public, &with_tag("ed25519-mldsa44")),
            Err(IdentityError::UnsupportedAlgorithm(
                "signature",
                "ed25519-mldsa44".to_string()
            ))
        );
        for tag in ["", "Ed25519", "ed 25519", &"a".repeat(33)] {
            assert_eq!(
                verify(Purpose::Login, V2, &public, &with_tag(tag)),
                Err(IdentityError::BadEncoding("signature")),
                "{tag:?}"
            );
        }
    }

    #[test]
    fn parse_rejects_oversized_payload() {
        let big = format!("\"{}\"", "a".repeat(16 * 1024));
        assert_eq!(
            parse::<String>(Purpose::Login, &big),
            Err(IdentityError::TooLarge { max: 16 * 1024 })
        );
        let card = format!("\"{}\"", "a".repeat(128 * 1024));
        assert_eq!(
            parse::<String>(Purpose::ProfileCard, &card),
            Err(IdentityError::TooLarge { max: 128 * 1024 })
        );
    }

    #[test]
    fn check_header_reads_both_versions_and_refuses_others() {
        assert_eq!(check_header(1, "login", "login"), Ok(V1));
        assert_eq!(check_header(2, "login", "login"), Ok(V2));
        assert_eq!(
            check_header(3, "login", "login"),
            Err(IdentityError::UnsupportedVersion(3))
        );
        assert_eq!(
            check_header(0, "login", "login"),
            Err(IdentityError::UnsupportedVersion(0))
        );
        assert_eq!(
            check_header(2, "device", "login"),
            Err(IdentityError::WrongType { expected: "login" })
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
