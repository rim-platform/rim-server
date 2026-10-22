//! Names the instance keeps for its own maintainers: signups and profile edits refuse them as
//! usernames and display names.

use std::sync::Arc;

use unicode_normalization::UnicodeNormalization;

use rim_services::{ServiceError, ServiceResult};

use crate::display_name::is_invisible;

/// The refusal of a reserved username or display name.
pub const RESERVED_NAME_REFUSAL: &str = "this name is reserved";

/// `name` without invisible characters, the joiners, variation selectors and tag characters a
/// stored name keeps among them (whitespace becomes a space), NFKC-normalised, lowercased and
/// trimmed: the form names are compared in.
fn fold(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some(' '),
            c if is_invisible(c) => None,
            c => Some(c),
        })
        .nfkc()
        .filter(|&c| !is_invisible(c))
        .collect::<String>()
        .to_lowercase()
        .trim()
        .to_string()
}

/// The Unicode confusable skeleton (Technical Standard 39) of a folded name, lowercased: names
/// that look alike, such as `admin` and `аdmin` with a Cyrillic `а`, share it.
fn skeleton(folded: &str) -> String {
    unicode_security::skeleton(folded)
        .collect::<String>()
        .to_lowercase()
}

/// Whether `name` is `reserved` or starts with it followed by a separator.
fn starts_with_reserved(name: &str, reserved: &str) -> bool {
    name.strip_prefix(reserved)
        .is_some_and(|rest| rest.chars().next().is_none_or(is_separator))
}

/// Whether `c` joins a reserved name to a suffix, as in `rim-admin`, `rim_admin`, `rim.admin`
/// or `rim admin`.
fn is_separator(c: char) -> bool {
    matches!(c, '-' | '_' | '.') || c.is_whitespace()
}

/// The reserved names, folded, and their skeletons. Empty by default: nothing is reserved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReservedNames {
    names: Arc<[String]>,
    skeletons: Arc<[String]>,
}

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
        let skeletons: Vec<String> = names.iter().map(|name| skeleton(name)).collect();
        Ok(Self {
            names: names.into(),
            skeletons: skeletons.into(),
        })
    }

    /// The folded names.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether `name`, folded, is a reserved name or starts with one followed by a separator,
    /// either as written or by its confusable skeleton, so a lookalike in another script is
    /// reserved too.
    pub fn is_reserved(&self, name: &str) -> bool {
        let name = fold(name);
        if self
            .names
            .iter()
            .any(|reserved| starts_with_reserved(&name, reserved))
        {
            return true;
        }
        let name = skeleton(&name);
        self.skeletons
            .iter()
            .any(|reserved| starts_with_reserved(&name, reserved))
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
    fn invisible_characters_hide_no_reserved_name() {
        let names = reserved("rim,admin,support");
        for name in [
            "Ad\u{AD}min",
            "Ad\u{200B}min",
            "\u{FEFF}Admin",
            "A\u{200D}dmin",
            "\u{202A}admin\u{202C}",
            "\u{2066}RIM\u{2069} Support",
            "r\u{200B}im",
            "rim\u{200B}",
            "rim\u{200B} team",
            "rim\tteam",
            "Ad\u{FE00}min",
            "Admin\u{FE0F}",
            "A\u{034F}dmin",
            "Ad\u{3164}min",
            "Ad\u{115F}min",
            "Ad\u{FFA0}min",
            "Ad\u{180E}min",
            "Ad\u{E0100}min",
            "Ad\u{E0020}min\u{E007F}",
            "Supp\u{FE00}ort",
            "RIM Supp\u{FE00}ort",
            "\u{3164}rim\u{3164}",
        ] {
            assert!(names.is_reserved(name), "{name:?}");
        }
        // Without the invisible character these read as one word, like "rimteam".
        for name in ["rim\u{200B}team", "rim\u{AD}team", "\u{202E}nimda"] {
            assert!(!names.is_reserved(name), "{name:?}");
        }
    }

    #[test]
    fn lookalike_letters_from_other_scripts_are_reserved() {
        let names = reserved("rim,admin,support");
        for name in [
            "\u{410}dmin",
            "\u{430}dmin",
            "R\u{456}m Support",
            "r\u{456}m",
            "\u{455}upport",
            "supp\u{43E}rt",
            "adrnin",
        ] {
            assert!(names.is_reserved(name), "{name:?}");
        }
        for name in ["rimmer", "r\u{456}mmer", "\u{430}dministrator", "Ада"] {
            assert!(!names.is_reserved(name), "{name:?}");
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
