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
fn is_control_or_format(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::Control | GeneralCategory::Format
    )
}

/// Whether `c` has the Unicode property Default_Ignorable_Code_Point (DerivedCoreProperties.txt):
/// a character that renders as nothing when a font has no glyph for it, such as a variation
/// selector, the combining grapheme joiner, a Hangul filler or a tag character.
fn is_default_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Whether `c` shows nothing itself: a control or format character or a default-ignorable one.
pub(crate) fn is_invisible(c: char) -> bool {
    is_control_or_format(c) || is_default_ignorable(c)
}

/// Whether `c` is the zero-width joiner or non-joiner: format characters that emoji sequences
/// and scripts such as Persian and Arabic need, which a stored display name keeps.
fn is_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

/// Whether `c` is a tag character of an emoji tag sequence, such as the subdivision code in the
/// flag of Scotland; [`CANCEL_TAG`] ends the sequence.
fn is_tag_spec(c: char) -> bool {
    matches!(c, '\u{E0020}'..='\u{E007E}')
}

/// The black flag that starts an emoji tag sequence.
const BLACK_FLAG: char = '\u{1F3F4}';

/// The tag character that ends an emoji tag sequence.
const CANCEL_TAG: char = '\u{E007F}';

/// Whether the invisible `c` stays in a stored name after `prev`, the character stored last
/// with no space since: the text emoji (U+FE0E) and emoji (U+FE0F) presentation selectors after
/// a visible character, and the Mongolian free variation selectors and vowel separator after a
/// Mongolian letter.
fn keeps_after(prev: Option<char>, c: char) -> bool {
    let Some(prev) = prev.filter(|&p| !is_invisible(p)) else {
        return false;
    };
    match c {
        '\u{FE0E}' | '\u{FE0F}' => true,
        '\u{180B}'..='\u{180F}' => matches!(prev, '\u{1800}'..='\u{18AF}'),
        _ => false,
    }
}

/// The length of the well-formed emoji tag sequence at the start of `rest`, the characters
/// after a black flag: tag characters ended by a cancel tag. Zero when there is none.
fn tag_sequence_len(rest: &[char]) -> usize {
    let specs = rest.iter().take_while(|&&c| is_tag_spec(c)).count();
    if specs > 0 && rest.get(specs) == Some(&CANCEL_TAG) {
        specs + 1
    } else {
        0
    }
}

/// `raw` with every invisible character removed but those emoji and scripts need, each run of
/// whitespace (a line break or a tab included) made one space, and no space or joiner at either
/// end. Kept are the zero-width joiner and non-joiner, a presentation selector after a visible
/// character, the tag characters of a flag such as Scotland's, and the Mongolian variation
/// selectors and vowel separator after a Mongolian letter. Empty when nothing visible is left.
pub(crate) fn clean(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut cleaned = String::with_capacity(raw.len());
    let mut pending_space = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        let prev = if pending_space {
            None
        } else {
            cleaned.chars().next_back()
        };
        if is_invisible(c) && !is_joiner(c) && !keeps_after(prev, c) {
            continue;
        }
        if pending_space && !cleaned.is_empty() {
            cleaned.push(' ');
        }
        pending_space = false;
        cleaned.push(c);
        if c == BLACK_FLAG {
            let tags = tag_sequence_len(&chars[i..]);
            cleaned.extend(&chars[i..i + tags]);
            i += tags;
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
/// cannot refuse, such as one in another server's profile card. The cut is cleaned again, so it
/// ends in no space, joiner or unfinished flag.
pub(crate) fn clean_truncated(raw: &str) -> String {
    let cut: String = clean(raw).chars().take(MAX_DISPLAY_NAME_CHARS).collect();
    clean(&cut)
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
    fn default_ignorable_characters_are_removed() {
        for (raw, cleaned) in [
            ("Ad\u{FE00}min", "Admin"),
            ("Ad\u{E0100}min", "Admin"),
            ("A\u{034F}dmin", "Admin"),
            ("Ad\u{3164}min", "Admin"),
            ("Ad\u{115F}\u{1160}min", "Admin"),
            ("Ad\u{FFA0}min", "Admin"),
            ("Ad\u{17B4}min", "Admin"),
            ("Ad\u{180B}min\u{180E}", "Admin"),
            ("Ad\u{E0020}min\u{E007F}", "Admin"),
            ("\u{FE0F}Admin", "Admin"),
            ("Admin \u{FE0F}", "Admin"),
            ("\u{200D}\u{FE0F}Ada", "Ada"),
            ("\u{2764}\u{FE0F}\u{FE0F}", "\u{2764}\u{FE0F}"),
            ("\u{1820}\u{180B}\u{180B}", "\u{1820}\u{180B}"),
            ("\u{3164}", ""),
            ("\u{3164} \u{FFA0}\u{FE0F}", ""),
        ] {
            assert_eq!(clean(raw), cleaned, "{raw:?}");
        }
    }

    #[test]
    fn emoji_presentation_flags_and_mongolian_selectors_are_kept() {
        for name in [
            // The flag of Scotland: a black flag, the tags "gbsct" and a cancel tag.
            "Ada \u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0077}\u{E006C}\u{E0073}\u{E007F} Cymru",
            "Ada \u{1F44D}\u{1F3FD}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{2764}\u{FE0F} Ada",
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
            "1\u{FE0F}\u{20E3}",
            "\u{263A}\u{FE0E}",
            // Mongolian "khara" with the vowel separator, and a letter with a variation selector.
            "\u{182C}\u{1820}\u{1837}\u{180E}\u{1820}",
            "\u{182D}\u{180B}\u{1820}",
        ] {
            assert_eq!(clean(name), name, "{name:?}");
        }
        // Tag characters outside a well-formed flag sequence are removed.
        for (raw, cleaned) in [
            ("\u{1F3F4}\u{E0067}\u{E0062}", "\u{1F3F4}"),
            ("\u{1F3F4}\u{E007F}", "\u{1F3F4}"),
            ("\u{1F3F4} \u{E0067}\u{E007F}", "\u{1F3F4}"),
            ("A\u{E0067}\u{E0062}\u{E007F}", "A"),
            ("Ada\u{180E}", "Ada"),
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
        // A cut through a flag or before a joiner leaves no unfinished sequence.
        let flag = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}";
        let x = "x".repeat(MAX_DISPLAY_NAME_CHARS - 4);
        assert_eq!(
            clean_truncated(&format!("{x}{flag}")),
            format!("{x}\u{1F3F4}")
        );
        let x = "x".repeat(MAX_DISPLAY_NAME_CHARS - 8);
        assert_eq!(clean_truncated(&format!("{x}{flag}")), format!("{x}{flag}"));
        let x = "x".repeat(MAX_DISPLAY_NAME_CHARS - 1);
        assert_eq!(clean_truncated(&format!("{x}\u{200D}y")), x);
    }
}
