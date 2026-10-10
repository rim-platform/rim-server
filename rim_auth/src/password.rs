use std::sync::OnceLock;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};

use crate::{AuthError, AuthResult};

/// Minimum password length enforced at the hashing boundary.
/// Any caller that bypasses this check would still fail here.
const MIN_PASSWORD_LENGTH: usize = 8;

/// Production-strength Argon2id parameters.
///
/// - Memory: 64 MiB (`m = 65536`)
/// - Iterations: 3 (`t = 3`)
/// - Parallelism: 4 (`p = 4`)
///
/// These meet the Jakub-specified production target. PHC string compatibility
/// means old hashes (hashed with the previous default m=19MiB, t=2, p=1) are
/// still verifiable by [`verify_password`] — the parameters are stored inside
/// the PHC string and Argon2 uses them during verification.
fn production_argon2() -> AuthResult<Argon2<'static>> {
    let params = Params::new(64 * 1024, 3, 4, None).map_err(|_| AuthError::PasswordHash)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hash `password` using Argon2id with production-strength parameters.
///
/// Returns a PHC-format string that embeds the algorithm, version, parameters,
/// salt, and hash — fully self-describing for future verification.
///
/// # Errors
/// - [`AuthError::InvalidCredentials`] when `password` is shorter than 8 characters.
/// - [`AuthError::PasswordHash`] on any internal hashing failure (e.g. entropy failure).
///
/// # Examples
/// ```ignore
/// let phc = hash_password("my_secret_password")?;
/// assert!(phc.starts_with("$argon2id"));
/// ```
pub fn hash_password(password: &str) -> AuthResult<String> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(AuthError::InvalidCredentials);
    }
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = production_argon2()?;
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::PasswordHash)?
        .to_string();
    Ok(hash)
}

/// Verify `password` against a PHC-format `password_hash`.
///
/// The verification algorithm, version, and parameters are read from the PHC
/// string itself, so hashes produced with different Argon2 parameters (e.g.
/// the weaker argon2 crate defaults) remain verifiable without any migration.
///
/// Returns `true` if the password matches, `false` otherwise.
///
/// # Errors
/// - [`AuthError::PasswordHash`] if `password_hash` is not a valid PHC string.
///
/// # Examples
/// ```ignore
/// let hash = hash_password("secret")?;
/// assert!(verify_password("secret", &hash)?);
/// assert!(!verify_password("wrong", &hash)?);
/// ```
pub fn verify_password(password: &str, password_hash: &str) -> AuthResult<bool> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| AuthError::PasswordHash)?;
    // Use the production instance for verification. Argon2 ignores the struct's
    // params when a PHC string embeds its own — so old low-param hashes still verify.
    let argon2 = production_argon2()?;
    Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
}

/// Run a full Argon2 verify of `password` against a fixed internal hash, discarding the result.
///
/// Call this on a "no such account" branch so it costs the same as a wrong password against a
/// real account; otherwise the missing hash work tells a caller which usernames exist. The
/// dummy hash is built once, with the production parameters, on first use; call this once at
/// startup so no request pays that extra cost.
///
/// # Errors
/// - [`AuthError::PasswordHash`] if the dummy hash cannot be built.
///
/// # Examples
/// ```ignore
/// verify_dummy_password("whatever the caller sent")?;
/// ```
pub fn verify_dummy_password(password: &str) -> AuthResult<()> {
    static DUMMY_HASH: OnceLock<Option<String>> = OnceLock::new();
    // `get_or_init` blocks concurrent first callers, so the hash is built exactly once.
    let hash = DUMMY_HASH
        .get_or_init(|| hash_password("rim-dummy-password-for-timing").ok())
        .as_deref()
        .ok_or(AuthError::PasswordHash)?;
    verify_password(password, hash)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_password_returns_valid_phc_string() {
        let hash = hash_password("my_secret").unwrap();
        assert!(
            hash.starts_with("$argon2"),
            "expected PHC string starting with $argon2, got: {hash}"
        );
    }

    /// The PHC string embeds the production parameters
    /// (m=65536, t=3, p=4) and uses Argon2id.
    #[test]
    fn hash_password_uses_production_argon2id_parameters() {
        let hash = hash_password("test_password_h2").unwrap();
        // PHC string format: $argon2id$v=19$m=65536,t=3,p=4$...
        assert!(
            hash.starts_with("$argon2id$"),
            "expected argon2id variant, got: {hash}"
        );
        assert!(
            hash.contains("m=65536"),
            "expected m=65536 (64 MiB), got: {hash}"
        );
        assert!(hash.contains("t=3"), "expected t=3, got: {hash}");
        assert!(hash.contains("p=4"), "expected p=4, got: {hash}");
    }

    /// Hashes produced with the argon2 crate's default params remain verifiable.
    ///
    /// This uses a pre-computed PHC string with m=19456 (old default ~19 MiB)
    /// to assert that `verify_password` transparently handles old hashes.
    #[test]
    fn verify_password_transparent_for_old_params() {
        // Pre-computed with argon2 default (m=19456, t=2, p=1), password = "oldpassword".
        // If this hash is ever regenerated, make sure to use the OLD params.
        // The string below was generated with: Argon2::default().hash_password(b"oldpassword", &salt)
        // We compute it dynamically here using old params to avoid hardcoding a salt.
        let old_params = Params::new(19 * 1024, 2, 1, None).expect("valid params");
        let old_argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, old_params);
        let salt = argon2::password_hash::SaltString::generate(&mut OsRng);
        let old_hash = old_argon2
            .hash_password(b"oldpassword", &salt)
            .unwrap()
            .to_string();

        // verify_password must accept a hash produced with the old (weaker) parameters.
        assert!(
            verify_password("oldpassword", &old_hash).unwrap(),
            "verify must succeed for hash produced with the default parameters"
        );
        assert!(
            !verify_password("wrongpassword", &old_hash).unwrap(),
            "wrong password must still fail"
        );
    }

    #[test]
    fn verify_password_returns_true_for_correct_password() {
        let hash = hash_password("test1234").unwrap();
        assert!(verify_password("test1234", &hash).unwrap());
    }

    #[test]
    fn verify_password_returns_false_for_wrong_password() {
        let hash = hash_password("test1234").unwrap();
        assert!(!verify_password("wrong_pw!", &hash).unwrap());
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        let h1 = hash_password("same_password").unwrap();
        let h2 = hash_password("same_password").unwrap();
        assert_ne!(
            h1, h2,
            "two hashes of the same password should differ due to random salt"
        );
    }

    #[test]
    fn verify_password_with_invalid_hash_format_returns_error() {
        let result = verify_password("test", "not-a-valid-hash");
        assert!(result.is_err());
    }

    #[test]
    fn hash_password_long_input() {
        let long_password = "a".repeat(1000);
        let hash = hash_password(&long_password).unwrap();
        assert!(verify_password(&long_password, &hash).unwrap());
    }

    #[test]
    fn hash_password_unicode() {
        let password = "p\u{00e4}ssw\u{00f6}rd \u{1f600}";
        let hash = hash_password(password).unwrap();
        assert!(verify_password(password, &hash).unwrap());
    }

    #[test]
    fn empty_password_is_rejected() {
        assert!(hash_password("").is_err());
    }

    #[test]
    fn short_password_is_rejected() {
        assert!(hash_password("1234567").is_err()); // 7 chars
    }

    #[test]
    fn verify_dummy_password_succeeds_for_any_input() {
        assert!(verify_dummy_password("").is_ok());
        assert!(verify_dummy_password("anything at all").is_ok());
    }

    #[test]
    fn minimum_length_password_is_accepted() {
        let hash = hash_password("12345678").unwrap(); // 8 chars
        assert!(verify_password("12345678", &hash).unwrap());
    }
}
