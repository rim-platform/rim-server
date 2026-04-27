use std::sync::LazyLock;

pub mod dto;
pub mod service;

pub use service::LinkPreviewService;

static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://[^\s<>\[\](){}|\\^`]+").unwrap());

/// Extracts all `http://` and `https://` URLs from a text string.
pub fn extract_urls(text: &str) -> Vec<String> {
    URL_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}
