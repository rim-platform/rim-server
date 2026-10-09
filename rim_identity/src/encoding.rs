use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::error::{IdentityError, IdentityResult};

/// Prefix of a key-based principal id. Provisional ids minted before key identity use `rimp0_`.
pub const PRINCIPAL_ID_PREFIX: &str = "rimp1_";

/// Domain separator hashed in front of the inception entry to form the principal id.
const PRINCIPAL_ID_DOMAIN: &[u8] = b"rim.principal.v1\0";

/// Length of a principal id: the prefix plus 43 characters of unpadded base64url SHA-256.
const PRINCIPAL_ID_LEN: usize = PRINCIPAL_ID_PREFIX.len() + 43;

pub(crate) fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode unpadded base64url into exactly `N` bytes.
///
/// The engine rejects padding and non-zero trailing bits, so every byte string has exactly one
/// accepted spelling and stored keys can be compared as text.
pub(crate) fn b64_fixed<const N: usize>(s: &str, what: &'static str) -> IdentityResult<[u8; N]> {
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| IdentityError::BadEncoding(what))?;
    bytes
        .try_into()
        .map_err(|_| IdentityError::BadEncoding(what))
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// An Ed25519 public key in its canonical encoding.
///
/// Parsing rejects weak (small-order) points and non-canonical encodings, so two different
/// strings never name the same key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// Parse a key from unpadded base64url. `what` names the field in errors.
    pub fn parse(s: &str, what: &'static str) -> IdentityResult<Self> {
        let bytes = b64_fixed::<32>(s, what)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| IdentityError::WeakKey(what))?;
        if key.is_weak() || key.to_edwards().compress().to_bytes() != bytes {
            return Err(IdentityError::WeakKey(what));
        }
        Ok(Self(key))
    }

    pub(crate) fn verifying_key(&self) -> &VerifyingKey {
        &self.0
    }
}

impl From<&SigningKey> for PublicKey {
    fn from(key: &SigningKey) -> Self {
        Self(key.verifying_key())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&b64(self.0.as_bytes()))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

/// The principal id fixed by an inception entry: `rimp1_` plus the unpadded base64url SHA-256
/// of a domain separator and the exact payload bytes.
///
/// The inception payload names both the first identity key and the recovery key, so the id
/// commits to the recovery key: a stolen identity key cannot mint a second log for the same id
/// with a recovery key of its own.
pub fn principal_id_for_inception(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PRINCIPAL_ID_DOMAIN);
    hasher.update(payload.as_bytes());
    format!("{PRINCIPAL_ID_PREFIX}{}", b64(&hasher.finalize()))
}

/// Whether `s` has the shape of a key-based principal id. Says nothing about whether it exists.
pub fn is_principal_id(s: &str) -> bool {
    s.len() == PRINCIPAL_ID_LEN
        && s.strip_prefix(PRINCIPAL_ID_PREFIX)
            .is_some_and(|rest| b64_fixed::<32>(rest, "principal id").is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_key_round_trips_through_its_text_form() {
        let key = PublicKey::from(&SigningKey::from_bytes(&[7; 32]));
        let parsed = PublicKey::parse(&key.to_string(), "key").expect("parse");
        assert_eq!(parsed, key);
    }

    #[test]
    fn public_key_rejects_padding_wrong_length_and_bad_alphabet() {
        let key = PublicKey::from(&SigningKey::from_bytes(&[7; 32])).to_string();
        assert!(PublicKey::parse(&format!("{key}="), "key").is_err());
        assert!(PublicKey::parse(&key[..42], "key").is_err());
        assert!(PublicKey::parse(&key.replace(|_: char| true, "+"), "key").is_err());
    }

    #[test]
    fn public_key_rejects_small_order_point() {
        // The identity point (y = 1) has order 1.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert_eq!(
            PublicKey::parse(&b64(&identity), "key"),
            Err(IdentityError::WeakKey("key"))
        );
    }

    #[test]
    fn public_key_rejects_non_canonical_encoding() {
        // y = p + 1 (p = 2^255 - 19) is a non-canonical spelling of y = 1.
        let mut non_canonical = [0xffu8; 32];
        non_canonical[0] = 0xee;
        non_canonical[31] = 0x7f;
        assert!(PublicKey::parse(&b64(&non_canonical), "key").is_err());
    }

    #[test]
    fn principal_id_has_prefix_and_fixed_length() {
        let id = principal_id_for_inception("{}");
        assert!(id.starts_with(PRINCIPAL_ID_PREFIX));
        assert!(is_principal_id(&id));
        assert_ne!(id, principal_id_for_inception("{} "));
    }

    #[test]
    fn is_principal_id_rejects_provisional_and_garbage() {
        assert!(!is_principal_id(
            "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU"
        ));
        assert!(!is_principal_id("rimp1_short"));
        assert!(!is_principal_id(""));
    }
}
