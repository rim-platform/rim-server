//! HMAC-signed CSRF tokens bound to the originating session.
//!
//! Token format: `rim_csrf_{nonce}.{signature}`
//! where:
//!   - `nonce`     = 32 random bytes, base64url-encoded (no padding)
//!   - `signature` = HMAC-SHA256(session_token, nonce_bytes), base64url-encoded
//!
//! The session token (from the HttpOnly cookie) acts as the HMAC key, binding
//! the CSRF token to a specific session.  An attacker who obtains a CSRF token
//! cannot reuse it with a different session.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const PREFIX: &str = "rim_csrf_";
const SEPARATOR: char = '.';

/// Generate a new CSRF token that is HMAC-bound to the given session token.
pub fn generate_csrf_token(session_token: &str) -> String {
    let nonce = crate::session_token::random_bytes::<32>("CSRF token");
    let nonce_b64 = URL_SAFE_NO_PAD.encode(nonce);

    let sig = compute_hmac(session_token, &nonce);
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);

    format!("{PREFIX}{nonce_b64}{SEPARATOR}{sig_b64}")
}

/// Verify that a CSRF token was signed with the given session token.
///
/// Returns `true` when the token has the expected format and the HMAC
/// signature is valid for the supplied session.
pub fn verify_csrf_token(csrf_token: &str, session_token: &str) -> bool {
    let body = match csrf_token.strip_prefix(PREFIX) {
        Some(b) => b,
        None => return false,
    };

    let (nonce_b64, sig_b64) = match body.split_once(SEPARATOR) {
        Some(pair) => pair,
        None => return false,
    };

    let nonce = match URL_SAFE_NO_PAD.decode(nonce_b64) {
        Ok(n) => n,
        Err(_) => return false,
    };

    let provided_sig = match URL_SAFE_NO_PAD.decode(sig_b64) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let expected_sig = compute_hmac(session_token, &nonce);

    // Constant-time comparison via the hmac crate's verify_slice.
    let mut mac =
        HmacSha256::new_from_slice(session_token.as_bytes()).expect("HMAC accepts any key size");
    mac.update(&nonce);
    // verify_slice performs constant-time comparison internally; it is given the
    // provided signature, not the expected one.
    drop(expected_sig);
    let mut verify_mac =
        HmacSha256::new_from_slice(session_token.as_bytes()).expect("HMAC accepts any key size");
    verify_mac.update(&nonce);
    verify_mac.verify_slice(&provided_sig).is_ok()
}

fn compute_hmac(session_token: &str, nonce: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(session_token.as_bytes()).expect("HMAC accepts any key size");
    mac.update(nonce);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_csrf_token_has_prefix() {
        let token = generate_csrf_token("rim_sess_test_session_123");
        assert!(
            token.starts_with("rim_csrf_"),
            "expected prefix rim_csrf_, got: {token}"
        );
    }

    #[test]
    fn generate_csrf_token_contains_separator() {
        let token = generate_csrf_token("rim_sess_test_session_123");
        let body = token.strip_prefix("rim_csrf_").unwrap();
        assert!(
            body.contains('.'),
            "expected separator '.' in token body, got: {body}"
        );
    }

    #[test]
    fn verify_csrf_token_succeeds_with_correct_session() {
        let session = "rim_sess_abc123xyz";
        let csrf = generate_csrf_token(session);
        assert!(
            verify_csrf_token(&csrf, session),
            "valid CSRF token should verify against its session"
        );
    }

    #[test]
    fn verify_csrf_token_fails_with_different_session() {
        let csrf = generate_csrf_token("rim_sess_session_A");
        assert!(
            !verify_csrf_token(&csrf, "rim_sess_session_B"),
            "CSRF token should NOT verify against a different session"
        );
    }

    #[test]
    fn verify_csrf_token_fails_with_empty_session() {
        let csrf = generate_csrf_token("rim_sess_real_session");
        assert!(
            !verify_csrf_token(&csrf, ""),
            "CSRF token should NOT verify against empty session"
        );
    }

    #[test]
    fn verify_csrf_token_fails_with_malformed_token() {
        assert!(!verify_csrf_token("rim_csrf_no_separator", "session"));
        assert!(!verify_csrf_token("wrong_prefix_abc.def", "session"));
        assert!(!verify_csrf_token("", "session"));
        assert!(!verify_csrf_token("rim_csrf_", "session"));
    }

    #[test]
    fn verify_csrf_token_fails_with_tampered_signature() {
        let session = "rim_sess_my_session";
        let csrf = generate_csrf_token(session);
        // Tamper with the last character of the signature.
        let tampered = if csrf.ends_with('A') {
            format!("{}B", &csrf[..csrf.len() - 1])
        } else {
            format!("{}A", &csrf[..csrf.len() - 1])
        };
        assert!(
            !verify_csrf_token(&tampered, session),
            "tampered CSRF token should fail verification"
        );
    }

    #[test]
    fn verify_csrf_token_with_empty_nonce() {
        // rim_csrf_.<valid_sig> - nonce portion is empty
        assert!(!verify_csrf_token("rim_csrf_.", "session"));
    }

    #[test]
    fn verify_csrf_token_with_invalid_base64_nonce() {
        assert!(!verify_csrf_token("rim_csrf_!!!invalid!!!.AAAA", "session"));
    }

    #[test]
    fn verify_csrf_token_with_invalid_base64_sig() {
        // Valid nonce but invalid sig
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1u8; 32]);
        let token = format!("rim_csrf_{nonce}.!!!invalid!!!");
        assert!(!verify_csrf_token(&token, "session"));
    }

    #[test]
    fn generate_csrf_token_with_empty_session() {
        // Should not panic, even with empty session key
        let token = generate_csrf_token("");
        assert!(token.starts_with("rim_csrf_"));
        // And it should verify against the same empty session
        assert!(verify_csrf_token(&token, ""));
    }

    #[test]
    fn two_tokens_for_same_session_are_unique() {
        let session = "rim_sess_same_session";
        let t1 = generate_csrf_token(session);
        let t2 = generate_csrf_token(session);
        assert_ne!(t1, t2, "two CSRF tokens should have different nonces");
        // But both should verify.
        assert!(verify_csrf_token(&t1, session));
        assert!(verify_csrf_token(&t2, session));
    }
}
