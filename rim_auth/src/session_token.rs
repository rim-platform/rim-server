use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use getrandom::fill;
use sha2::{Digest, Sha256};

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    if let Err(e) = fill(&mut bytes) {
        // getrandom failure is an OS-level fault (entropy source unavailable).
        // Log to the audit channel before unwinding so the failure is visible
        // in forensics even if the logging worker guard drops during panic.
        tracing::error!(
            target = "audit",
            error = %e,
            "fatal: OS entropy source failed; cannot generate session token"
        );
        panic!("getrandom failed: {e}");
    }
    let token = URL_SAFE_NO_PAD.encode(bytes);
    format!("rim_sess_{token}")
}

/// Generate a new alert-ingest webhook token: 32 random bytes, base64url-nopad,
/// prefixed `rim_ingest_`. Presented once to the caller at creation time; only
/// its [`hash_token`] digest is ever persisted (mirrors [`generate_session_token`]).
pub fn generate_ingest_token() -> String {
    let mut bytes = [0u8; 32];
    if let Err(e) = fill(&mut bytes) {
        tracing::error!(
            target = "audit",
            error = %e,
            "fatal: OS entropy source failed; cannot generate ingest token"
        );
        panic!("getrandom failed: {e}");
    }
    let token = URL_SAFE_NO_PAD.encode(bytes);
    format!("rim_ingest_{token}")
}

/// Generate a key-login challenge nonce: 32 random bytes, base64url-nopad, no prefix. It is
/// public (the client signs it), so it is stored as is rather than hashed.
pub fn generate_challenge_nonce() -> String {
    let mut bytes = [0u8; 32];
    if let Err(e) = fill(&mut bytes) {
        tracing::error!(
            target = "audit",
            error = %e,
            "fatal: OS entropy source failed; cannot generate challenge nonce"
        );
        panic!("getrandom failed: {e}");
    }
    URL_SAFE_NO_PAD.encode(bytes)
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
    fn generate_ingest_token_has_prefix() {
        let token = generate_ingest_token();
        assert!(
            token.starts_with("rim_ingest_"),
            "expected prefix rim_ingest_, got: {token}"
        );
    }

    #[test]
    fn generate_ingest_token_has_sufficient_length() {
        let token = generate_ingest_token();
        let after_prefix = token.strip_prefix("rim_ingest_").unwrap();
        assert!(
            after_prefix.len() >= 43,
            "expected at least 43 chars after prefix (256-bit base64url), got {}",
            after_prefix.len()
        );
    }

    #[test]
    fn two_ingest_tokens_are_unique() {
        let t1 = generate_ingest_token();
        let t2 = generate_ingest_token();
        assert_ne!(t1, t2, "two generated tokens should be unique");
    }

    #[test]
    fn ingest_token_hashes_via_shared_hash_token() {
        let token = generate_ingest_token();
        let h1 = hash_token(&token);
        let h2 = hash_token(&token);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
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
