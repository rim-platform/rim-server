//! The form every display name takes before it is stored, whoever set it: a signup, a profile
//! edit or a profile card from another server.

use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use rim_services::{ServiceError, ServiceResult};

/// Most characters a stored display name holds, the width of its column.
pub(crate) const MAX_DISPLAY_NAME_CHARS: usize = 64;

/// The refusal of a display name longer than [`MAX_DISPLAY_NAME_CHARS`].
pub(crate) const DISPLAY_NAME_TOO_LONG: &str = "display_name must be at most 64 characters";

/// Whether `c` is a control (Cc) or format (Cf) character: one that shows nothing itself but can
/// hide, join or reorder the text around it, such as a zero-width space, a soft hyphen, a
/// byte order mark or a bidirectional override.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::Control | GeneralCategory::Format
    )
}

/// Whether `c` is the zero-width joiner or non-joiner: format characters that emoji sequences
/// and scripts such as Persian and Arabic need, which a stored display name keeps.
fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// `raw` with every control and format character but the joiners removed, each run of
/// whitespace (a line break or a tab included) made one space, and no space or joiner at either
/// end. Empty when nothing visible is left.
pub(crate) fn clean(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = true;
        } else if !is_invisible(c) || is_joiner(c) {
            if pending_space && !cleaned.is_empty() {
                cleaned.push(' ');
            }
            pending_space = false;
            cleaned.push(c);
        }
    }
    cleaned
        .trim_matches(|c| c == ' ' || is_joiner(c))
        .to_string()
}

/// `raw` [`clean`]ed, for a display name this server's own users set.
///
/// # Errors
/// `Validation` ([`DISPLAY_NAME_TOO_LONG`]) when the cleaned name is longer than
/// [`MAX_DISPLAY_NAME_CHARS`].
pub(crate) fn clean_local(raw: &str) -> ServiceResult<String> {
    let cleaned = clean(raw);
    if cleaned.chars().count() > MAX_DISPLAY_NAME_CHARS {
        return Err(ServiceError::Validation(DISPLAY_NAME_TOO_LONG.to_string()));
    }
    Ok(cleaned)
}

/// `raw` [`clean`]ed and cut to [`MAX_DISPLAY_NAME_CHARS`], for a display name this server
/// cannot refuse, such as one in another server's profile card.
pub(crate) fn clean_truncated(raw: &str) -> String {
    let cut: String = clean(raw).chars().take(MAX_DISPLAY_NAME_CHARS).collect();
    cut.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_control_characters_are_removed() {
        for (raw, cleaned) in [
            ("Ad\u{AD}min", "Admin"),
            ("Ad\u{200B}min", "Admin"),
            ("Adm\u{200E}i\u{200F}n", "Admin"),
            ("\u{202A}Admin\u{202C}", "Admin"),
            ("\u{202E}nimda", "nimda"),
            ("\u{2066}RIM\u{2069} Support", "RIM Support"),
            ("\u{FEFF}Admin", "Admin"),
            ("Al\u{0}ice\u{7}", "Alice"),
            ("Ad\u{2060}min\u{E0041}", "Admin"),
        ] {
            assert_eq!(clean(raw), cleaned, "{raw:?}");
        }
    }

    #[test]
    fn whitespace_runs_become_one_space_and_the_ends_are_trimmed() {
        for (raw, cleaned) in [
            ("  Alice   Smith  ", "Alice Smith"),
            ("Alice\nSmith", "Alice Smith"),
            ("Alice\t\r\nSmith", "Alice Smith"),
            ("Alice \u{200B} Smith", "Alice Smith"),
            ("Alice\u{3000}Smith", "Alice Smith"),
            ("\u{200B} \u{FEFF}", ""),
            ("", ""),
        ] {
            assert_eq!(clean(raw), cleaned, "{raw:?}");
        }
    }

    #[test]
    fn the_joiners_are_kept_for_emoji_sequences_and_scripts_that_need_them() {
        for name in [
            "Ada \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}",
            "Ad\u{200D}min",
        ] {
            assert_eq!(clean(name), name, "{name:?}");
        }
        // A joiner at either end joins nothing.
        for (raw, cleaned) in [
            ("\u{200D}", ""),
            (" \u{200C}\u{200D} ", ""),
            ("\u{200D} Ada \u{200C}", "Ada"),
        ] {
            assert_eq!(clean(raw), cleaned, "{raw:?}");
        }
    }

    #[test]
    fn letters_marks_and_emoji_are_kept() {
        for name in ["Zoë", "Ада Лавлейс", "李小龍", "José 🎉", "Ⓐlice"] {
            assert_eq!(clean(name), name);
        }
    }

    #[test]
    fn a_local_name_is_capped_after_cleaning() {
        let at_cap = "é".repeat(MAX_DISPLAY_NAME_CHARS);
        assert_eq!(clean_local(&at_cap).unwrap(), at_cap);
        let padded = format!("{}\u{200B}\u{200B}", "x".repeat(MAX_DISPLAY_NAME_CHARS));
        assert_eq!(clean_local(&padded).unwrap().chars().count(), 64);
        assert!(matches!(
            clean_local(&"x".repeat(MAX_DISPLAY_NAME_CHARS + 1)),
            Err(ServiceError::Validation(m)) if m == DISPLAY_NAME_TOO_LONG
        ));
    }

    #[test]
    fn a_remote_name_is_cut_to_the_cap() {
        let long = format!("{} tail", "x".repeat(MAX_DISPLAY_NAME_CHARS - 1));
        assert_eq!(
            clean_truncated(&long),
            "x".repeat(MAX_DISPLAY_NAME_CHARS - 1)
        );
        assert_eq!(clean_truncated("\u{202E}Ada"), "Ada");
    }
}
