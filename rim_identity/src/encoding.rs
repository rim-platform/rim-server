use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::error::{IdentityError, IdentityResult};
use crate::statement::StatementVersion;

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

/// Separates the algorithm name from the base64url body in a tagged key or signature. It is not
/// in the base64url alphabet, so the first one splits the text unambiguously.
const TAG_SEPARATOR: char = ':';

/// Longest algorithm name accepted in a tag.
const MAX_ALGORITHM_NAME_LEN: usize = 32;

/// A signature algorithm named by a tagged key or signature (`<name>:<base64url>`).
///
/// Ed25519 is the only algorithm implemented today. The hybrid Ed25519 + ML-DSA-44 scheme
/// (`docs/decisions/post-quantum-hybrid-2026-10-09.md`) arrives as a new variant; until then
/// its name is refused like any other unknown name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Algorithm {
    Ed25519,
}

impl Algorithm {
    /// The name written in front of a tagged key or signature.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ed25519 => "ed25519",
        }
    }

    fn from_name(name: &str, what: &'static str) -> IdentityResult<Self> {
        match name {
            "ed25519" => Ok(Self::Ed25519),
            _ => Err(IdentityError::UnsupportedAlgorithm(what, name.to_string())),
        }
    }
}

/// Split `<name>:<body>` into the algorithm and the base64url body.
///
/// A name that is empty, too long or not `[a-z0-9-]` is an encoding error; a well-formed name
/// this crate does not implement is [`IdentityError::UnsupportedAlgorithm`], so a client can
/// tell "newer than this server" from "garbage".
fn split_tagged<'a>(s: &'a str, what: &'static str) -> IdentityResult<(Algorithm, &'a str)> {
    let (name, body) = s
        .split_once(TAG_SEPARATOR)
        .ok_or(IdentityError::BadEncoding(what))?;
    let well_formed = !name.is_empty()
        && name.len() <= MAX_ALGORITHM_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !well_formed {
        return Err(IdentityError::BadEncoding(what));
    }
    Ok((Algorithm::from_name(name, what)?, body))
}

/// The algorithm and body of a key or signature as spelled by a statement of `version`:
/// version 1 has no tag and means Ed25519, version 2 is always tagged.
fn split_for_version<'a>(
    version: StatementVersion,
    s: &'a str,
    what: &'static str,
) -> IdentityResult<(Algorithm, &'a str)> {
    match version {
        StatementVersion::V1 => Ok((Algorithm::Ed25519, s)),
        StatementVersion::V2 => split_tagged(s, what),
    }
}

#[derive(Clone, PartialEq, Eq)]
enum KeyMaterial {
    Ed25519(VerifyingKey),
}

/// A public key of any supported [`Algorithm`].
///
/// Its canonical text form, used by version 2 statements, by the server's storage and by the
/// GraphQL API, is tagged: `ed25519:<unpadded base64url>`. Version 1 statements spell Ed25519
/// keys without the tag. Parsing rejects weak (small-order) points and non-canonical encodings,
/// so two different strings in one form never name the same key. Two keys are equal when their
/// algorithm and key bytes are, whichever form they were read from.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(KeyMaterial);

impl PublicKey {
    /// Parse a key in its canonical tagged form. `what` names the field in errors.
    pub fn parse(s: &str, what: &'static str) -> IdentityResult<Self> {
        Self::parse_for(StatementVersion::V2, s, what)
    }

    /// Parse a key as spelled in a statement of `version`.
    pub fn parse_for(
        version: StatementVersion,
        s: &str,
        what: &'static str,
    ) -> IdentityResult<Self> {
        let (algorithm, body) = split_for_version(version, s, what)?;
        match algorithm {
            Algorithm::Ed25519 => Self::parse_ed25519(body, what),
        }
    }

    fn parse_ed25519(body: &str, what: &'static str) -> IdentityResult<Self> {
        let bytes = b64_fixed::<32>(body, what)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| IdentityError::WeakKey(what))?;
        if key.is_weak() || key.to_edwards().compress().to_bytes() != bytes {
            return Err(IdentityError::WeakKey(what));
        }
        Ok(Self(KeyMaterial::Ed25519(key)))
    }

    pub fn algorithm(&self) -> Algorithm {
        match self.0 {
            KeyMaterial::Ed25519(_) => Algorithm::Ed25519,
        }
    }

    /// The key as a statement of `version` spells it. Version 1 can only spell Ed25519 keys.
    pub fn encode_for(&self, version: StatementVersion) -> String {
        match (&self.0, version) {
            (KeyMaterial::Ed25519(key), StatementVersion::V1) => b64(key.as_bytes()),
            (_, StatementVersion::V2) => self.to_string(),
        }
    }

    /// The raw key bytes, without the algorithm tag.
    pub fn key_bytes(&self) -> Vec<u8> {
        match &self.0 {
            KeyMaterial::Ed25519(key) => key.as_bytes().to_vec(),
        }
    }

    /// SHA-256 of the canonical tagged text. Servers index device keys by it, so the index
    /// stays small whatever the key size; the key-hash migration computes the same value in SQL.
    pub fn key_hash(&self) -> [u8; 32] {
        sha256(self.to_string().as_bytes())
    }

    /// Check a raw signature (no tag, no base64) over `message`, in this key's algorithm, with
    /// the strict checks. For signature formats that carry the algorithm elsewhere, such as an
    /// HTTP message signature's `alg` parameter.
    pub fn verify_bytes(&self, message: &[u8], signature: &[u8]) -> IdentityResult<()> {
        let signature = match self.0 {
            KeyMaterial::Ed25519(_) => Signature::Ed25519(ed25519_dalek::Signature::from_bytes(
                signature
                    .try_into()
                    .map_err(|_| IdentityError::BadEncoding("signature"))?,
            )),
        };
        self.verify(message, &signature)
    }

    /// Check `signature` over `message` with the strict checks that reject malleable
    /// signatures and small-order keys.
    pub(crate) fn verify(&self, message: &[u8], signature: &Signature) -> IdentityResult<()> {
        match (&self.0, signature) {
            (KeyMaterial::Ed25519(key), Signature::Ed25519(sig)) => key
                .verify_strict(message, sig)
                .map_err(|_| IdentityError::BadSignature),
        }
    }
}

impl From<&SigningKey> for PublicKey {
    fn from(key: &SigningKey) -> Self {
        Self(KeyMaterial::Ed25519(key.verifying_key()))
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            KeyMaterial::Ed25519(key) => write!(
                f,
                "{}{TAG_SEPARATOR}{}",
                Algorithm::Ed25519.name(),
                b64(key.as_bytes())
            ),
        }
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

/// A decoded signature of any supported [`Algorithm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Signature {
    Ed25519(ed25519_dalek::Signature),
}

impl Signature {
    /// Decode a signature as spelled by a statement of `version` and check that its algorithm
    /// is `key`'s, so a signature never verifies under a key of another algorithm.
    pub(crate) fn decode_for(
        version: StatementVersion,
        s: &str,
        key: &PublicKey,
    ) -> IdentityResult<Self> {
        let (algorithm, body) = split_for_version(version, s, "signature")?;
        if algorithm != key.algorithm() {
            return Err(IdentityError::AlgorithmMismatch);
        }
        match algorithm {
            Algorithm::Ed25519 => Ok(Self::Ed25519(ed25519_dalek::Signature::from_bytes(
                &b64_fixed::<64>(body, "signature")?,
            ))),
        }
    }

    /// The signature as a statement of `version` spells it.
    pub(crate) fn encode_for(&self, version: StatementVersion) -> String {
        match (self, version) {
            (Self::Ed25519(sig), StatementVersion::V1) => b64(&sig.to_bytes()),
            (Self::Ed25519(sig), StatementVersion::V2) => format!(
                "{}{TAG_SEPARATOR}{}",
                Algorithm::Ed25519.name(),
                b64(&sig.to_bytes())
            ),
        }
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
    use crate::statement::StatementVersion::{V1, V2};

    fn key() -> PublicKey {
        PublicKey::from(&SigningKey::from_bytes(&[7; 32]))
    }

    #[test]
    fn public_key_round_trips_through_its_text_forms() {
        let key = key();
        let tagged = key.to_string();
        assert!(tagged.starts_with("ed25519:"));
        assert_eq!(key.encode_for(V2), tagged);
        assert_eq!(PublicKey::parse(&tagged, "key"), Ok(key.clone()));
        assert_eq!(PublicKey::parse_for(V2, &tagged, "key"), Ok(key.clone()));

        let bare = key.encode_for(V1);
        assert_eq!(format!("ed25519:{bare}"), tagged);
        assert_eq!(PublicKey::parse_for(V1, &bare, "key"), Ok(key.clone()));
        assert_eq!(key.algorithm(), Algorithm::Ed25519);
    }

    #[test]
    fn each_version_accepts_only_its_own_spelling() {
        let key = key();
        assert_eq!(
            PublicKey::parse_for(V1, &key.to_string(), "key"),
            Err(IdentityError::BadEncoding("key"))
        );
        assert_eq!(
            PublicKey::parse_for(V2, &key.encode_for(V1), "key"),
            Err(IdentityError::BadEncoding("key"))
        );
    }

    #[test]
    fn public_key_rejects_padding_wrong_length_and_bad_alphabet() {
        let bare = key().encode_for(V1);
        for body in [
            format!("{bare}="),
            bare[..42].to_string(),
            bare.replace(|_: char| true, "+"),
        ] {
            assert!(PublicKey::parse_for(V1, &body, "key").is_err(), "{body}");
            assert!(PublicKey::parse(&format!("ed25519:{body}"), "key").is_err());
        }
    }

    #[test]
    fn tagged_key_with_unknown_or_malformed_algorithm_is_refused() {
        let bare = key().encode_for(V1);
        assert_eq!(
            PublicKey::parse(&format!("ed25519-mldsa44:{bare}"), "deviceKey"),
            Err(IdentityError::UnsupportedAlgorithm(
                "deviceKey",
                "ed25519-mldsa44".to_string()
            ))
        );
        for tag in ["", "ED25519", "ed_25519", " ed25519", &"x".repeat(33)] {
            assert_eq!(
                PublicKey::parse(&format!("{tag}:{bare}"), "deviceKey"),
                Err(IdentityError::BadEncoding("deviceKey")),
                "{tag:?}"
            );
        }
        // The first colon splits, so a second one lands in the body and fails there.
        assert!(PublicKey::parse(&format!("ed25519::{bare}"), "deviceKey").is_err());
    }

    #[test]
    fn public_key_rejects_small_order_point() {
        // The identity point (y = 1) has order 1.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert_eq!(
            PublicKey::parse_for(V1, &b64(&identity), "key"),
            Err(IdentityError::WeakKey("key"))
        );
        assert_eq!(
            PublicKey::parse(&format!("ed25519:{}", b64(&identity)), "key"),
            Err(IdentityError::WeakKey("key"))
        );
    }

    #[test]
    fn public_key_rejects_non_canonical_encoding() {
        // y = p + 1 (p = 2^255 - 19) is a non-canonical spelling of y = 1.
        let mut non_canonical = [0xffu8; 32];
        non_canonical[0] = 0xee;
        non_canonical[31] = 0x7f;
        assert!(PublicKey::parse_for(V1, &b64(&non_canonical), "key").is_err());
    }

    #[test]
    fn key_hash_is_sha256_of_the_tagged_text() {
        let key = key();
        assert_eq!(key.key_hash(), sha256(key.to_string().as_bytes()));
        let other = PublicKey::from(&SigningKey::from_bytes(&[8; 32]));
        assert_ne!(key.key_hash(), other.key_hash());
    }

    #[test]
    fn signature_round_trips_in_both_spellings() {
        let key = key();
        let sig = Signature::Ed25519(ed25519_dalek::Signature::from_bytes(&[0; 64]));
        let tagged = sig.encode_for(V2);
        assert_eq!(Signature::decode_for(V2, &tagged, &key), Ok(sig.clone()));
        assert_eq!(
            Signature::decode_for(V1, &sig.encode_for(V1), &key),
            Ok(sig)
        );
    }

    #[test]
    fn verify_bytes_checks_raw_signatures_strictly() {
        use ed25519_dalek::Signer;
        let signing = SigningKey::from_bytes(&[7; 32]);
        let sig = signing.sign(b"message").to_bytes();
        assert_eq!(key().verify_bytes(b"message", &sig), Ok(()));
        assert_eq!(
            key().verify_bytes(b"other", &sig),
            Err(IdentityError::BadSignature)
        );
        assert_eq!(
            key().verify_bytes(b"message", &sig[..63]),
            Err(IdentityError::BadEncoding("signature"))
        );
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
