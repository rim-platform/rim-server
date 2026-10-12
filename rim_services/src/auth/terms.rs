//! The terms-of-service rule every signup follows.

use crate::{ServiceError, ServiceResult};

/// The refusal of a signup that did not accept the instance's terms.
pub const TERMS_NOT_ACCEPTED: &str =
    "the terms of service must be accepted: send the server's terms URL as the accepted terms";

/// The terms URL a signup records on the new account. `None` when the instance has no terms,
/// whatever the signup sent; the instance's URL when `accepted` equals it.
///
/// # Errors
/// `Validation` when the instance has terms and `accepted` is missing or names another URL.
pub fn accepted_terms(
    terms_url: Option<&str>,
    accepted: Option<&str>,
) -> ServiceResult<Option<String>> {
    match terms_url {
        None => Ok(None),
        Some(url) if accepted == Some(url) => Ok(Some(url.to_string())),
        Some(_) => Err(ServiceError::Validation(TERMS_NOT_ACCEPTED.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMS: &str = "https://rim.example.org/terms";

    fn refused(result: ServiceResult<Option<String>>) -> bool {
        matches!(result, Err(ServiceError::Validation(m)) if m == TERMS_NOT_ACCEPTED)
    }

    #[test]
    fn a_signup_without_the_accepted_terms_is_refused() {
        assert!(refused(accepted_terms(Some(TERMS), None)));
    }

    #[test]
    fn a_signup_accepting_other_terms_is_refused() {
        assert!(refused(accepted_terms(
            Some(TERMS),
            Some("https://rim.example.org/old-terms")
        )));
        assert!(refused(accepted_terms(Some(TERMS), Some(""))));
    }

    #[test]
    fn a_signup_accepting_the_terms_records_them() {
        assert_eq!(
            accepted_terms(Some(TERMS), Some(TERMS)).ok(),
            Some(Some(TERMS.to_string()))
        );
    }

    #[test]
    fn without_terms_the_input_is_ignored() {
        assert_eq!(accepted_terms(None, None).ok(), Some(None));
        assert_eq!(accepted_terms(None, Some(TERMS)).ok(), Some(None));
    }
}
