//! Names the instance keeps for its own maintainers: signups and profile edits refuse them as
//! usernames and display names.

use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;

use rim_services::{ServiceError, ServiceResult};

/// The refusal of a reserved username or display name.
pub const RESERVED_NAME_REFUSAL: &str = "this name is reserved";

/// `name` trimmed, NFKC-normalised and lowercased: the form names are compared in.
fn fold(name: &str) -> String {
    name.trim().nfkc().collect::<String>().to_lowercase()
}

/// Whether `c` joins a reserved name to a suffix, as in `rim-admin`, `rim_admin`, `rim.admin`
/// or `rim admin`.
fn is_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.') || c.is_whitespace()
}

/// The reserved names, folded. Empty by default: nothing is reserved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReservedNames(Arc<[String]>);

impl ReservedNames {
    /// The names of a comma-separated list; a blank list reserves nothing.
    ///
    /// # Errors
    /// A message naming the position of an entry that is empty after trimming.
    pub fn parse(list: &str) -> Result<Self, String> {
        if list.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut names: Vec<String> = Vec::new();
        for (index, entry) in list.split(',').enumerate() {
            let name = fold(entry);
            if name.is_empty() {
                return Err(format!("entry {} is empty", index + 1));
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(Self(names.into()))
    }

    /// The folded names.
    pub fn names(&self) -> &[String] {
        &self.0
    }

    /// Whether `name`, folded, is a reserved name or starts with one followed by a separator.
    pub fn is_reserved(&self, name: &str) -> bool {
        let name = fold(name);
        self.0.iter().any(|reserved| {
            name.strip_prefix(reserved.as_str())
                .is_some_and(|rest| rest.chars().next().is_none_or(is_separator))
        })
    }

    /// # Errors
    /// `Validation` ([`RESERVED_NAME_REFUSAL`]) when `name` is reserved.
    pub fn check(&self, name: &str) -> ServiceResult<()> {
        if self.is_reserved(name) {
            return Err(ServiceError::Validation(RESERVED_NAME_REFUSAL.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reserved(list: &str) -> ReservedNames {
        ReservedNames::parse(list).expect("a valid list")
    }

    #[test]
    fn a_blank_list_reserves_nothing_and_an_empty_entry_is_refused() {
        for blank in ["", "  "] {
            let names = reserved(blank);
            assert!(names.names().is_empty());
            assert!(!names.is_reserved("rim"));
        }
        for (list, position) in [
            ("rim,,admin", 2),
            ("rim,", 2),
            (" , rim", 1),
            ("rim, ,x", 2),
        ] {
            assert_eq!(
                ReservedNames::parse(list),
                Err(format!("entry {position} is empty")),
                "{list}"
            );
        }
    }

    #[test]
    fn entries_are_trimmed_folded_and_kept_once() {
        assert_eq!(
            reserved(" Rim , ADMIN,rim ,Ｓｕｐｐｏｒｔ").names(),
            ["rim", "admin", "support"]
        );
    }

    #[test]
    fn names_match_regardless_of_case_space_around_and_compatibility_forms() {
        let names = reserved("rim,admin");
        for name in ["rim", "RIM", "  Rim ", "ｒｉｍ", "ＡＤＭＩＮ", "Admin"] {
            assert!(names.is_reserved(name), "{name}");
        }
    }

    #[test]
    fn a_reserved_name_followed_by_a_separator_is_reserved_too() {
        let names = reserved("rim");
        for name in [
            "rim-admin",
            "rim_admin",
            "rim.admin",
            "Rim Team",
            "RIM-",
            "rim\u{3000}team",
        ] {
            assert!(names.is_reserved(name), "{name}");
        }
    }

    #[test]
    fn a_name_that_only_starts_or_ends_with_the_letters_is_not_reserved() {
        let names = reserved("rim");
        for name in [
            "rimmer",
            "Rimbaud",
            "prim",
            "trim-admin",
            "ri-m",
            "admin-rim",
            "",
        ] {
            assert!(!names.is_reserved(name), "{name}");
        }
    }

    #[test]
    fn check_answers_validation_with_the_refusal() {
        let names = reserved("rim");
        assert!(names.check("rimmer").is_ok());
        assert!(matches!(
            names.check("rim-admin"),
            Err(ServiceError::Validation(m)) if m == RESERVED_NAME_REFUSAL
        ));
    }
}
