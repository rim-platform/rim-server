use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use crate::{AuthError, AuthResult};

pub fn hash_password(password: &str) -> AuthResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::PasswordHash)?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, password_hash: &str) -> AuthResult<bool> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| AuthError::PasswordHash)?;
    let argon2 = Argon2::default();
    Ok(argon2.verify_password(password.as_bytes(), &parsed).is_ok())
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

    #[test]
    fn verify_password_returns_true_for_correct_password() {
        let hash = hash_password("test123").unwrap();
        assert!(verify_password("test123", &hash).unwrap());
    }

    #[test]
    fn verify_password_returns_false_for_wrong_password() {
        let hash = hash_password("test123").unwrap();
        assert!(!verify_password("wrong", &hash).unwrap());
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
    fn empty_password_can_be_hashed_and_verified() {
        let hash = hash_password("").unwrap();
        assert!(verify_password("", &hash).unwrap());
    }
}
