//! Handles: `@<username>:<domain>`, a person's username at a server and the handle domain that
//! server declares in its descriptor.

/// Longest handle domain, as for a DNS name.
pub const MAX_HANDLE_DOMAIN_LEN: usize = 253;

/// Longest label of a handle domain.
const MAX_LABEL_LEN: usize = 63;

/// Whether `domain` is a handle domain: a lowercase DNS name of at most 253 bytes whose labels
/// are 1 to 63 ASCII letters, digits and hyphens, neither starting nor ending with a hyphen.
/// A single label (`localhost`) is accepted; a trailing dot is not.
pub fn is_handle_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= MAX_HANDLE_DOMAIN_LEN
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL_LEN
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The handle of `username` at `domain`: `@<username>:<domain>`.
pub fn format_handle(username: &str, domain: &str) -> String {
    format!("@{username}:{domain}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_domains_are_lowercase_dns_names() {
        for ok in [
            "rim-platform.org",
            "relay.rim-platform.org",
            "localhost",
            "127.0.0.1",
            "a.b-c.d0",
            &format!("{}.org", "a".repeat(63)),
        ] {
            assert!(is_handle_domain(ok), "{ok}");
        }
        for bad in [
            "",
            "Rim-Platform.org",
            "rim-platform.org.",
            ".rim-platform.org",
            "rim..org",
            "-rim.org",
            "rim-.org",
            "rim_platform.org",
            "rim platform.org",
            "[::1]",
            "rim-platform.org:443",
            "https://rim-platform.org",
            "ří.cz",
            &format!("{}.org", "a".repeat(64)),
            &["a"; 128].join("."),
        ] {
            assert!(!is_handle_domain(bad), "{bad}");
        }
    }

    #[test]
    fn a_handle_joins_username_and_domain() {
        assert_eq!(
            format_handle("kuba", "rim-platform.org"),
            "@kuba:rim-platform.org"
        );
    }
}
