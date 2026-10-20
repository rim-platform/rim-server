//! The terms-of-service and minimum-age rules every signup follows.

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

/// The refusal of a signup that did not confirm the instance's minimum age.
pub const MINIMUM_AGE_NOT_CONFIRMED: &str = "the minimum age must be confirmed: send the server's \
     minimum age as the confirmed minimum age once the person confirms reaching it";

/// The minimum age a signup records on the new account. `None` when the instance asks for no
/// age, whatever the signup sent; the instance's age when `confirmed` equals it.
///
/// # Errors
/// `Validation` when the instance has a minimum age and `confirmed` is missing or another
/// number.
pub fn confirmed_minimum_age(
    minimum_age: Option<u8>,
    confirmed: Option<i32>,
) -> ServiceResult<Option<u8>> {
    match minimum_age {
        None => Ok(None),
        Some(age) if confirmed == Some(i32::from(age)) => Ok(Some(age)),
        Some(_) => Err(ServiceError::Validation(
            MINIMUM_AGE_NOT_CONFIRMED.to_string(),
        )),
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

    fn age_refused(result: ServiceResult<Option<u8>>) -> bool {
        matches!(result, Err(ServiceError::Validation(m)) if m == MINIMUM_AGE_NOT_CONFIRMED)
    }

    #[test]
    fn a_signup_without_the_minimum_age_or_with_another_is_refused() {
        assert!(age_refused(confirmed_minimum_age(Some(16), None)));
        assert!(age_refused(confirmed_minimum_age(Some(16), Some(13))));
        assert!(age_refused(confirmed_minimum_age(Some(16), Some(18))));
    }

    #[test]
    fn a_signup_confirming_the_minimum_age_records_it() {
        assert_eq!(
            confirmed_minimum_age(Some(16), Some(16)).ok(),
            Some(Some(16))
        );
    }

    #[test]
    fn without_a_minimum_age_the_confirmation_is_ignored() {
        assert_eq!(confirmed_minimum_age(None, None).ok(), Some(None));
        assert_eq!(confirmed_minimum_age(None, Some(16)).ok(), Some(None));
    }

    #[test]
    fn the_age_refusal_does_not_read_as_a_terms_refusal() {
        assert!(!MINIMUM_AGE_NOT_CONFIRMED.to_lowercase().contains("terms"));
    }

    #[test]
    fn without_terms_the_input_is_ignored() {
        assert_eq!(accepted_terms(None, None).ok(), Some(None));
        assert_eq!(accepted_terms(None, Some(TERMS)).ok(), Some(None));
    }
}
