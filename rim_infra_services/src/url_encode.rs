//! Minimal, dependency-free percent-encoding for a single URL path segment (Track B3).
//!
//! Reset/invite tokens are already restricted to `[A-Za-z0-9_-]` (base64url-nopad), which are
//! all RFC 3986 "unreserved" characters and never need encoding — but §5 of
//! `docs/specs/track-b3-email.md` asks for defensive encoding anyway, and pulling in a full URL
//! crate for that would be scope creep for one helper. This encodes exactly the RFC 3986
//! unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) as-is and percent-encodes everything
//! else.

/// Percent-encode `segment` for safe inclusion as a single path segment.
///
/// # Examples
/// ```ignore
/// use rim_infra_services::url_encode::percent_encode_path_segment;
/// assert_eq!(percent_encode_path_segment("rim_rst_abc123"), "rim_rst_abc123");
/// assert_eq!(percent_encode_path_segment("a/b c"), "a%2Fb%20c");
/// ```
pub(crate) fn percent_encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_characters_pass_through_unchanged() {
        let token = "rim_rst_Ab12-CdEf_09.~";
        assert_eq!(percent_encode_path_segment(token), token);
    }

    #[test]
    fn reserved_and_unsafe_characters_are_percent_encoded() {
        assert_eq!(percent_encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(percent_encode_path_segment("a b"), "a%20b");
        assert_eq!(percent_encode_path_segment("a?b=c"), "a%3Fb%3Dc");
        assert_eq!(percent_encode_path_segment("100%"), "100%25");
    }

    #[test]
    fn empty_string_encodes_to_empty_string() {
        assert_eq!(percent_encode_path_segment(""), "");
    }
}
