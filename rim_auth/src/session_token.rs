use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use getrandom::fill;
use sha2::{Digest, Sha256};

/// Generate a session token: `rim_sess_` and 32 random bytes, base64url-nopad.
pub fn generate_session_token() -> String {
    generate_token("rim_sess_")
}

/// Generate a bearer token: `prefix` and 32 random bytes, base64url-nopad. Only its SHA-256
/// ([`hash_token`]) is ever persisted.
pub fn generate_token(prefix: &str) -> String {
    format!("{prefix}{}", random_b64url::<32>("token"))
}

/// Generate a key-login challenge nonce: 32 random bytes, base64url-nopad, no prefix. It is
/// public (the client signs it), so it is stored as is rather than hashed.
pub fn generate_challenge_nonce() -> String {
    random_b64url::<32>("challenge nonce")
}

/// Generate a device link id: 16 random bytes, base64url-nopad (22 characters), no prefix.
/// It is a bearer capability, so only its SHA-256 is ever persisted.
pub fn generate_link_id() -> String {
    random_b64url::<16>("device link id")
}

/// The prefix of a workspace's public join link token, which tells it apart from an invite token.
pub const PUBLIC_LINK_TOKEN_PREFIX: &str = "rim_pjl_";

/// Generate a workspace's public join link token: [`PUBLIC_LINK_TOKEN_PREFIX`] and 16 random
/// bytes, base64url-nopad (22 characters). Only the SHA-256 of the whole string
/// ([`hash_token`]) is looked up; the server keeps the token sealed for its Owner.
pub fn generate_public_link_token() -> String {
    format!(
        "{PUBLIC_LINK_TOKEN_PREFIX}{}",
        random_b64url::<16>("public link token")
    )
}

/// `N` bytes from the OS random generator, base64url-nopad. `what` names the value in the log
/// line written before panicking when the entropy source fails.
fn random_b64url<const N: usize>(what: &str) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>(what))
}

/// `N` bytes from the OS random generator. An entropy failure is an OS-level fault: it is
/// logged to the audit channel, naming `what`, and then panics.
pub(crate) fn random_bytes<const N: usize>(what: &str) -> [u8; N] {
    let mut bytes = [0u8; N];
    if let Err(e) = fill(&mut bytes) {
        tracing::error!(
            target: "audit",
            error = %e,
            "fatal: OS entropy source failed; cannot generate {what}"
        );
        panic!("getrandom failed: {e}");
    }
    bytes
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let out = hasher.finalize();
    hex_encode(&out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut res = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        res.push(HEX[(b >> 4) as usize] as char);
        res.push(HEX[(b & 0x0f) as usize] as char);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_session_token_has_prefix() {
        let token = generate_session_token();
        assert!(
            token.starts_with("rim_sess_"),
            "expected prefix rim_sess_, got: {token}"
        );
    }

    #[test]
    fn generate_session_token_has_sufficient_length() {
        let token = generate_session_token();
        let after_prefix = token.strip_prefix("rim_sess_").unwrap();
        assert!(
            after_prefix.len() >= 43,
            "expected at least 43 chars after prefix (256-bit base64url), got {}",
            after_prefix.len()
        );
    }

    #[test]
    fn token_is_the_prefix_and_32_random_bytes_of_unpadded_base64url() {
        let token = generate_token("rim_test_");
        let random = token.strip_prefix("rim_test_").expect("prefix");
        assert_eq!(URL_SAFE_NO_PAD.decode(random).map(|b| b.len()), Ok(32));
        assert_ne!(token, generate_token("rim_test_"));
    }

    #[test]
    fn public_link_token_is_its_prefix_and_16_random_bytes_of_unpadded_base64url() {
        let token = generate_public_link_token();
        let random = token.strip_prefix("rim_pjl_").expect("prefix");
        assert_eq!(random.len(), 22);
        assert_eq!(URL_SAFE_NO_PAD.decode(random).map(|b| b.len()), Ok(16));
        assert_ne!(token, generate_public_link_token());
    }

    #[test]
    fn link_id_is_16_random_bytes_of_unpadded_base64url() {
        let id = generate_link_id();
        assert_eq!(id.len(), 22);
        assert_eq!(URL_SAFE_NO_PAD.decode(&id).map(|b| b.len()), Ok(16));
        assert_ne!(id, generate_link_id());
    }

    #[test]
    fn two_tokens_are_unique() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        assert_ne!(t1, t2, "two generated tokens should be unique");
    }

    #[test]
    fn hash_token_is_consistent() {
        let token = generate_session_token();
        let h1 = hash_token(&token);
        let h2 = hash_token(&token);
        assert_eq!(
            h1, h2,
            "hashing the same token twice should produce identical results"
        );
    }

    #[test]
    fn hash_token_differs_for_different_inputs() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        let h1 = hash_token(&t1);
        let h2 = hash_token(&t2);
        assert_ne!(h1, h2, "hashes of different tokens should differ");
    }

    #[test]
    fn hash_token_of_empty_string() {
        let hash = hash_token("");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hex_encode_empty() {
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn hex_encode_known_values() {
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0xab, 0xcd]), "abcd");
        assert_eq!(hex_encode(&[0x01, 0x23, 0x45, 0x67, 0x89]), "0123456789");
    }

    #[test]
    fn generate_session_token_is_url_safe() {
        let token = generate_session_token();
        let after_prefix = token.strip_prefix("rim_sess_").unwrap();
        // base64url chars: A-Z, a-z, 0-9, -, _
        assert!(
            after_prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token should only contain URL-safe base64 characters, got: {after_prefix}"
        );
    }

    #[test]
    fn hash_token_is_hex_sha256() {
        let token = generate_session_token();
        let hash = hash_token(&token);
        assert_eq!(
            hash.len(),
            64,
            "SHA-256 hex digest should be 64 chars, got {}",
            hash.len()
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should contain only hex characters, got: {hash}"
        );
    }
}
