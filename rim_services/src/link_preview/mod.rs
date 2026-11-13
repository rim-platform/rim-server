use std::sync::LazyLock;

pub mod dto;
pub mod service;

pub use service::LinkPreviewService;

/// Preview fetches one process runs at once when `RIM_LINK_PREVIEW_CONCURRENCY` is unset.
pub const DEFAULT_FETCH_CONCURRENCY: usize = 8;

/// Most preview fetches one process may run at once.
pub const MAX_FETCH_CONCURRENCY: usize = 64;

static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://[^\s<>\[\]{}|\\^`]+").unwrap());

/// Extracts all `http://` and `https://` URLs from a text string.
///
/// Sentence punctuation (`.,;:!?'"*_~`) after a URL is not part of it, and a closing
/// parenthesis is dropped unless an opening one inside the URL balances it, so
/// `(see https://example.com/a)` yields `https://example.com/a` and
/// `https://example.com/a_(b)` keeps its parentheses.
pub fn extract_urls(text: &str) -> Vec<String> {
    URL_RE
        .find_iter(text)
        .map(|m| trim_url_end(m.as_str()).to_string())
        .filter(|url| url.len() > "https://".len())
        .collect()
}

/// Cuts trailing punctuation and unbalanced closing parentheses off a matched URL.
fn trim_url_end(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let candidate = &url[..end];
        let Some(last) = candidate.chars().next_back() else {
            return candidate;
        };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~' => true,
            ')' => candidate.matches(')').count() > candidate.matches('(').count(),
            _ => false,
        };
        if !drop {
            return candidate;
        }
        end -= last.len_utf8();
    }
}

#[cfg(test)]
mod tests {
    use super::extract_urls;

    #[test]
    fn trailing_period_is_not_part_of_the_url() {
        assert_eq!(
            extract_urls("read https://example.com/code-of-conduct.html."),
            ["https://example.com/code-of-conduct.html"]
        );
    }

    #[test]
    fn trailing_comma_is_not_part_of_the_url() {
        assert_eq!(
            extract_urls("https://example.com/a, then https://example.com/b!"),
            ["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn closing_parenthesis_of_the_sentence_is_dropped() {
        assert_eq!(
            extract_urls("(see https://example.com/a)."),
            ["https://example.com/a"]
        );
    }

    #[test]
    fn balanced_parentheses_inside_the_url_are_kept() {
        assert_eq!(
            extract_urls("https://en.wikipedia.org/wiki/Rust_(language)."),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
    }

    #[test]
    fn bare_scheme_is_not_a_url() {
        assert!(extract_urls("https://. and http://,").is_empty());
    }
}
